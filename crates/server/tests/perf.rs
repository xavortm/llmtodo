//! Times the issue list on a large data set. It is ignored in normal runs.
//!
//! ```sh
//! cargo test --release -p llmtodo-server --test perf -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sqlx::Connection as _;

const ISSUES: i64 = 50_000;

async fn timed_list(url: &str, http: &reqwest::Client, limit: u32) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let start = Instant::now();
        let res = http
            .post(format!("{url}/v1/issues/query"))
            .json(&json!({ "limit": limit }))
            .send()
            .await
            .unwrap();
        assert!(res.status().is_success());
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["items"].as_array().unwrap().len(), limit as usize);
        best = best.min(start.elapsed());
    }
    best
}

#[tokio::test]
#[ignore = "builds a large database"]
async fn list_with_large_dependency_graphs() {
    let dir = tempfile::tempdir().unwrap();
    let running = llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.path().to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap();
    let url = running.url();
    let http = reqwest::Client::new();
    let team = http
        .post(format!("{url}/v1/teams"))
        .json(&json!({ "key": "ALX", "name": "Alex" }))
        .send()
        .await
        .unwrap();
    assert!(team.status().is_success());
    let first = http
        .post(format!("{url}/v1/issues"))
        .json(&json!({ "title": "Template" }))
        .send()
        .await
        .unwrap();
    assert!(first.status().is_success());

    let mut db = sqlx::SqliteConnection::connect(&format!(
        "sqlite://{}",
        dir.path().join("llmtodo.db").display()
    ))
    .await
    .unwrap();
    // Copies the template row. The IDs sort in number order.
    sqlx::query(
        "WITH RECURSIVE n(v) AS (SELECT 2 UNION ALL SELECT v + 1 FROM n WHERE v < ?)
         INSERT INTO issues (id, team_id, number, key, title, description, status_id, wanted_executor, created_at, updated_at)
         SELECT printf('perf-%08d', n.v), i.team_id, n.v, 'ALX-' || n.v, 'Issue ' || n.v, '', i.status_id, '{}', i.created_at, i.updated_at
         FROM n, issues i WHERE i.number = 1",
    )
    .bind(ISSUES)
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO issue_keys (key, issue_id) SELECT key, id FROM issues WHERE number > 1",
    )
    .execute(&mut db)
    .await
    .unwrap();

    let no_usage = timed_list(&url, &http, 1000).await;
    println!("{ISSUES} issues, no usage, page of 1000: {no_usage:?}");

    // Three agent sessions per issue, each with usage on two models.
    sqlx::query(
        "INSERT INTO actors (id, kind, name, created_at)
         VALUES ('perf-agent', 'agent', 'perf', '2026-01-01T00:00:00Z')",
    )
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "WITH RECURSIVE s(v) AS (SELECT 1 UNION ALL SELECT v + 1 FROM s WHERE v < 3)
         INSERT INTO work_sessions (id, session_id, issue_id, actor_id, started_at, last_seen_at)
         SELECT 'ws-' || s.v || '-' || i.id, 'session-' || s.v, i.id, 'perf-agent', i.created_at, i.created_at
         FROM s, issues i",
    )
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO work_session_usage (work_session_id, model, requests, input, output, cache_read)
         SELECT ws.id, m.model, 10, 1000, 500, 90000
         FROM work_sessions ws, (SELECT 'claude-opus-5-5' AS model UNION ALL SELECT 'claude-sonnet-5-5') m",
    )
    .execute(&mut db)
    .await
    .unwrap();
    let with_usage = timed_list(&url, &http, 1000).await;
    println!("150k work sessions with usage, page of 1000: {with_usage:?}");
    let with_usage_small = timed_list(&url, &http, 100).await;
    println!("150k work sessions with usage, page of 100: {with_usage_small:?}");

    let no_relations = timed_list(&url, &http, 1000).await;
    println!("{ISSUES} issues, no relations, page of 1000: {no_relations:?}");

    // Chains of 20: each issue waits for the one before it.
    sqlx::query(
        "INSERT INTO relations (id, type, from_id, to_id, created_at)
         SELECT 'r-' || a.id, 'blocks', a.id, b.id, a.created_at
         FROM issues a JOIN issues b ON b.number = a.number + 1
         WHERE a.number > 1 AND a.number % 20 <> 0",
    )
    .execute(&mut db)
    .await
    .unwrap();
    let chains = timed_list(&url, &http, 1000).await;
    println!("47.5k edges in chains of 20, page of 1000: {chains:?}");
    let chains_small = timed_list(&url, &http, 100).await;
    println!("47.5k edges in chains of 20, page of 100: {chains_small:?}");

    // The worst shape: one chain through every issue, with extra edges that
    // share blockers. Counting paths in SQL would never finish on this.
    sqlx::query(
        "INSERT OR IGNORE INTO relations (id, type, from_id, to_id, created_at)
         SELECT 'x-' || a.id, 'blocks', a.id, b.id, a.created_at
         FROM issues a JOIN issues b ON b.number = a.number + 1
         WHERE a.number > 1 AND a.number % 20 = 0",
    )
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT OR IGNORE INTO relations (id, type, from_id, to_id, created_at)
         SELECT 'y-' || a.id, 'blocks', a.id, b.id, a.created_at
         FROM issues a JOIN issues b ON b.number = a.number + 2
         WHERE a.number > 1",
    )
    .execute(&mut db)
    .await
    .unwrap();
    // The newest issues sort first, and they sit at the end of the chain, so
    // the page reaches every issue before it.
    sqlx::query("UPDATE issues SET updated_at = '2099-01-01T00:00:00Z' WHERE number > ?")
        .bind(ISSUES - 1000)
        .execute(&mut db)
        .await
        .unwrap();
    let worst = timed_list(&url, &http, 1000).await;
    println!("one chain through all {ISSUES} issues, page of 1000: {worst:?}");

    db.close().await.unwrap();
    running.stop().await.unwrap();
}
