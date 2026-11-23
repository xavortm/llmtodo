//! Tests for the shared Postgres backend (LLM-450). They run only when
//! `LLMTODO_TEST_POSTGRES_URL` is set, for example against the database in
//! `compose.yaml`:
//!
//! ```sh
//! docker compose up -d --wait postgres
//! LLMTODO_TEST_POSTGRES_URL=postgres://llmtodo:llmtodo@127.0.0.1:54329/llmtodo \
//!     cargo test -p llmtodo-server --test postgres
//! ```
//!
//! Every other server test also runs on Postgres with that variable set.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use base64::Engine;
use futures_util::StreamExt;
use llmtodo_api_types::*;
use llmtodo_server::store::backend::{self, Pool};
use serde_json::{Value, json};

fn admin_url() -> Option<String> {
    std::env::var("LLMTODO_TEST_POSTGRES_URL")
        .ok()
        .filter(|u| !u.is_empty())
}

macro_rules! needs_postgres {
    ($admin:ident) => {
        let Some($admin) = admin_url() else {
            eprintln!("skipped: set LLMTODO_TEST_POSTGRES_URL to run the Postgres tests");
            return;
        };
        let _ = &$admin;
    };
}

/// One server on a shared database, with its own data folder.
struct Server {
    url: String,
    http: reqwest::Client,
    running: llmtodo_server::Running,
    _dir: tempfile::TempDir,
}

impl Server {
    async fn start(db_url: &str, host: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let running = llmtodo_server::start(llmtodo_server::Config {
            data_dir: dir.path().to_path_buf(),
            port: 0,
            host: Some(host.into()),
            github_poll: false,
            database: llmtodo_server::Database::Postgres {
                url: db_url.to_string(),
            },
        })
        .await
        .unwrap();
        Server {
            url: running.url(),
            http: reqwest::Client::new(),
            running,
            _dir: dir,
        }
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

    async fn ok(&self, method: &str, path: &str, body: Value) -> Value {
        let (s, v) = self.call(&owner(), method, path, body).await;
        assert!(s < 300, "{method} {path} -> {s}: {v}");
        v
    }

    async fn issue(&self, title: &str) -> String {
        self.ok(
            "POST",
            "/v1/issues",
            json!({ "title": title, "estimate_minutes": 5, "estimate_cost_usd": 1.0 }),
        )
        .await["key"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn stop(self) {
        self.running.stop().await.unwrap();
    }
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

/// Two servers on one fresh database, with one team.
async fn pair() -> (Server, Server) {
    let admin = admin_url().unwrap();
    let url = llmtodo_server::db::create_test_database(&admin)
        .await
        .unwrap();
    let a = Server::start(&url, "laptop-a").await;
    let b = Server::start(&url, "laptop-b").await;
    a.ok(
        "POST",
        "/v1/teams",
        json!({ "key": "LLM", "name": "llmtodo" }),
    )
    .await;
    (a, b)
}

/// Table name to its columns, without the search tables, which are FTS5
/// virtual tables on SQLite, and the bookkeeping tables.
async fn tables(pool: &Pool) -> BTreeMap<String, BTreeSet<String>> {
    let rows: Vec<(String, String)> = match pool.backend() {
        backend::Backend::Sqlite => backend::query_as(
            "SELECT m.name, p.name FROM sqlite_master m, pragma_table_info(m.name) p
             WHERE m.type = 'table'",
        )
        .fetch_all(pool)
        .await
        .unwrap(),
        backend::Backend::Postgres => backend::query_as(
            "SELECT table_name::text, column_name::text FROM information_schema.columns
             WHERE table_schema = current_schema() AND is_generated = 'NEVER'",
        )
        .fetch_all(pool)
        .await
        .unwrap(),
    };
    let skip = |t: &str| {
        t.starts_with("search_fts")
            || t.starts_with("issues_trgm")
            || t.starts_with("sqlite_")
            || t == "_sqlx_migrations"
    };
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (t, c) in rows.into_iter().filter(|(t, _)| !skip(t)) {
        out.entry(t).or_default().insert(c);
    }
    out
}

/// LLM-453: both schemas hold the same tables and columns, so one set of
/// queries and row types serves both.
#[tokio::test]
async fn both_schemas_have_the_same_tables_and_columns() {
    needs_postgres!(admin);
    let dir = tempfile::tempdir().unwrap();
    let sqlite = llmtodo_server::db::open_sqlite_file(&dir.path().join("s.db"))
        .await
        .unwrap();
    let url = llmtodo_server::db::create_test_database(&admin)
        .await
        .unwrap();
    let pg = llmtodo_server::db::open_postgres(&url).await.unwrap();
    assert_eq!(tables(&sqlite).await, tables(&pg).await);
}

/// LLM-457: two servers on one database claim one task at the same moment.
/// Exactly one wins, and the other hears that the claim is held.
#[tokio::test]
async fn two_servers_claim_one_task_and_one_wins() {
    needs_postgres!(admin);
    let (a, b) = pair().await;
    for round in 0..5 {
        let k = a.issue(&format!("Contested {round}")).await;
        let path = format!("/v1/issues/{k}/claim");
        let (who_a, who_b) = (agent(&format!("a-{round}")), agent(&format!("b-{round}")));
        let (ra, rb) = tokio::join!(
            a.call(&who_a, "POST", &path, json!({})),
            b.call(&who_b, "POST", &path, json!({})),
        );
        let wins = [ra.0, rb.0].iter().filter(|s| **s < 300).count();
        assert_eq!(wins, 1, "round {round}: {ra:?} {rb:?}");
        let loser = if ra.0 < 300 { &rb.1 } else { &ra.1 };
        assert_eq!(loser["error"]["code"], "claim_held", "{loser}");
    }
    a.stop().await;
    b.stop().await;
}

/// LLM-457: issues created on two servers at once never share a key.
#[tokio::test]
async fn two_servers_create_issues_with_unique_keys() {
    needs_postgres!(admin);
    let (a, b) = pair().await;
    let mut tasks = Vec::new();
    for n in 0..10 {
        let (a_url, b_url) = (a.url.clone(), b.url.clone());
        tasks.push(tokio::spawn(async move {
            let url = if n % 2 == 0 { a_url } else { b_url };
            let res = reqwest::Client::new()
                .post(format!("{url}/v1/issues"))
                .json(&json!({ "title": format!("Task {n}") }))
                .send()
                .await
                .unwrap();
            assert!(res.status().is_success());
            res.json::<Value>().await.unwrap()["key"]
                .as_str()
                .unwrap()
                .to_string()
        }));
    }
    let mut keys = BTreeSet::new();
    for t in tasks {
        assert!(keys.insert(t.await.unwrap()), "a key repeated");
    }
    assert_eq!(keys.len(), 10);
    a.stop().await;
    b.stop().await;
}

async fn next_event(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Value {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("an event within 10 seconds")
            .unwrap()
            .unwrap();
        if let tokio_tungstenite::tungstenite::Message::Text(t) = msg {
            let v: Value = serde_json::from_str(&t).unwrap();
            if v["type"] == "event" {
                return v;
            }
        }
    }
}

/// LLM-459: a write on one server reaches a live client of the other, in
/// seq order with no gap.
#[tokio::test]
async fn a_write_on_one_server_reaches_live_clients_of_the_other() {
    needs_postgres!(admin);
    let (a, b) = pair().await;
    let ws_url = format!("{}/v1/ws", b.url.replacen("http://", "ws://", 1));
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    let k = a.issue("Made on laptop A").await;
    let first = next_event(&mut ws).await;
    assert_eq!(first["event"]["issue"], k);
    let mut last = first["event"]["seq"].as_i64().unwrap();

    // Writes on both servers, interleaved: B's client hears all of them.
    for n in 0..6 {
        let s = if n % 2 == 0 { &a } else { &b };
        s.ok(
            "PATCH",
            &format!("/v1/issues/{k}"),
            json!({ "title": format!("Title {n}") }),
        )
        .await;
    }
    let mut titles = Vec::new();
    while titles.len() < 6 {
        let e = next_event(&mut ws).await;
        let seq = e["event"]["seq"].as_i64().unwrap();
        assert!(seq > last, "seq {seq} after {last}");
        last = seq;
        if e["event"]["field"] == "title" {
            titles.push(e["event"]["new"].as_str().unwrap().to_string());
        }
    }
    let want: Vec<String> = (0..6).map(|n| format!("Title {n}")).collect();
    assert_eq!(titles, want);
    a.stop().await;
    b.stop().await;
}

/// LLM-460: a backup of a Postgres database is a SQLite file with the same
/// rows, which a SQLite server can open.
#[tokio::test]
async fn a_postgres_backup_is_a_sqlite_file_with_every_row() {
    needs_postgres!(admin);
    let (a, b) = pair().await;
    b.stop().await;
    for n in 0..3 {
        a.issue(&format!("Kept {n}")).await;
    }
    let path = a.ok("POST", "/v1/server/backup", json!({})).await["path"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(path.ends_with(".db"), "{path}");
    let copy = llmtodo_server::db::open_sqlite_file(std::path::Path::new(&path))
        .await
        .unwrap();
    let n: i64 = backend::query_scalar("SELECT count(*) FROM issues")
        .fetch_one(&copy)
        .await
        .unwrap();
    assert_eq!(n, 3);
    let found: i64 =
        backend::query_scalar("SELECT count(*) FROM search_fts WHERE search_fts MATCH '\"kept\"'")
            .fetch_one(&copy)
            .await
            .unwrap();
    assert_eq!(found, 3, "the copy's search tables are filled");
    copy.close().await;
    a.stop().await;
}

/// LLM-458: a file one server stored is served by the other, from the
/// shared database.
#[tokio::test]
async fn a_file_stored_through_one_server_is_served_by_the_other() {
    needs_postgres!(admin);
    let (a, b) = pair().await;
    let bytes = b"\x89PNG not really".to_vec();
    let hash = "ab".repeat(32);
    let mut c = a.running.state.pool().acquire().await.unwrap();
    llmtodo_server::store::import::insert_file(
        &mut c,
        &hash,
        Some("image/png".into()),
        bytes.len() as i64,
        "https://uploads.linear.app/x.png",
        Some(bytes.clone()),
    )
    .await
    .unwrap();
    drop(c);
    let res = b
        .http
        .get(format!("{}/v1/files/{hash}", b.url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "image/png");
    assert_eq!(res.bytes().await.unwrap().to_vec(), bytes);
    a.stop().await;
    b.stop().await;
}

/// LLM-460: one server at a time holds a job lock, such as the GitHub
/// poll's, and another takes it once the first lets go.
#[tokio::test]
async fn one_server_holds_a_job_lock_at_a_time() {
    needs_postgres!(admin);
    let (a, b) = pair().await;
    let (pa, pb) = (a.running.state.pool(), b.running.state.pool());
    let held = pa.try_job_lock(42).await.unwrap();
    assert!(held.is_some());
    assert!(pb.try_job_lock(42).await.unwrap().is_none());
    // Dropping the guard closes its connection. Postgres frees the lock as
    // it sees the close.
    drop(held);
    let mut taken = None;
    for _ in 0..50 {
        taken = pb.try_job_lock(42).await.unwrap();
        if taken.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(taken.is_some(), "the lock never freed");
    drop(taken);
    a.stop().await;
    b.stop().await;
}
