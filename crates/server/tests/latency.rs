//! Times the common requests on a Postgres database a network hop away
//! (LLM-465). A proxy between the server and Postgres delays each packet,
//! so every statement costs a round trip, as it does on Neon. It is ignored
//! in normal runs, and needs `LLMTODO_TEST_POSTGRES_URL`:
//!
//! ```sh
//! docker compose up -d --wait postgres
//! LLMTODO_TEST_POSTGRES_URL=postgres://llmtodo:llmtodo@127.0.0.1:54329/llmtodo \
//!     cargo test --release -p llmtodo-server --test latency -- --ignored --nocapture
//! ```
//!
//! `LLMTODO_LATENCY_MS` sets the round trip, 40 by default, and
//! `LLMTODO_LATENCY_ISSUES` the number of issues, 50,000 by default.
//! `LLMTODO_SQL_TRACE=1` prints every statement the server runs.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use llmtodo_server::store::backend::STATEMENTS;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn env_num(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Copies bytes one way, holding each chunk for `delay` first. Chunks keep
/// their order, so a burst costs one delay, as on a real link.
async fn pipe(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    delay: Duration,
) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
    tokio::spawn(async move {
        while let Some((at, chunk)) = rx.recv().await {
            tokio::time::sleep_until((at + delay).into()).await;
            if to.write_all(&chunk).await.is_err() {
                return;
            }
        }
    });
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match from.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                if tx.send((Instant::now(), buf[..n].to_vec())).is_err() {
                    return;
                }
            }
        }
    }
}

/// A TCP proxy to `target` that adds `rtt` to every round trip. Returns
/// its port.
async fn delay_proxy(target: String, rtt: Duration) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let one_way = rtt / 2;
    tokio::spawn(async move {
        loop {
            let Ok((client, _)) = listener.accept().await else {
                return;
            };
            let target = target.clone();
            tokio::spawn(async move {
                let Ok(server) = TcpStream::connect(&target).await else {
                    return;
                };
                client.set_nodelay(true).ok();
                server.set_nodelay(true).ok();
                let (cr, cw) = client.into_split();
                let (sr, sw) = server.into_split();
                tokio::spawn(pipe(cr, sw, one_way));
                tokio::spawn(pipe(sr, cw, one_way));
            });
        }
    });
    port
}

/// The URL with its host and port replaced by the proxy's.
fn through(url: &str, port: u16) -> (String, String) {
    let (scheme, rest) = url.split_once("://").unwrap();
    let (auth, hostpath) = rest.rsplit_once('@').unwrap();
    let (hostport, path) = hostpath.split_once('/').unwrap();
    (
        format!("{scheme}://{auth}@127.0.0.1:{port}/{path}"),
        hostport.to_string(),
    )
}

struct Timed {
    what: String,
    ms: f64,
    statements: u64,
}

async fn timed<F: std::future::Future<Output = Value>>(
    out: &mut Vec<Timed>,
    what: &str,
    f: F,
) -> Value {
    let before = STATEMENTS.load(Ordering::Relaxed);
    let start = Instant::now();
    let v = f.await;
    out.push(Timed {
        what: what.to_string(),
        ms: start.elapsed().as_secs_f64() * 1000.0,
        statements: STATEMENTS.load(Ordering::Relaxed) - before,
    });
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "builds a large database and waits on a slow link"]
async fn requests_on_a_remote_database() {
    let Some(admin) = std::env::var("LLMTODO_TEST_POSTGRES_URL")
        .ok()
        .filter(|u| !u.is_empty())
    else {
        eprintln!("skipped: set LLMTODO_TEST_POSTGRES_URL");
        return;
    };
    let rtt = Duration::from_millis(env_num("LLMTODO_LATENCY_MS", 40));
    let issues = env_num("LLMTODO_LATENCY_ISSUES", 50_000) as i64;

    let url = llmtodo_server::db::create_test_database(&admin)
        .await
        .unwrap();
    // Fill the database over a direct link, then serve it through the slow
    // one.
    let direct = llmtodo_server::db::open_postgres(&url, None).await.unwrap();
    let (_, target) = through(&url, 0);
    let port = delay_proxy(target, rtt).await;
    let (slow_url, _) = through(&url, port);

    let dir = tempfile::tempdir().unwrap();
    let running = llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.path().to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
        offline_copy: false,
        database: llmtodo_server::Database::Postgres { url: slow_url },
    })
    .await
    .unwrap();
    let base = running.url();
    let http = reqwest::Client::new();
    let who = {
        use base64::Engine;
        let ctx = llmtodo_api_types::ClientContext {
            actor: llmtodo_api_types::ActorInput {
                kind: llmtodo_api_types::ActorKind::Agent,
                name: None,
                harness: Some("claude-code".into()),
                model: Some("opus-5.5".into()),
                effort: Some("high".into()),
            },
            session_id: Some("latency".into()),
            ..Default::default()
        };
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&ctx).unwrap())
    };
    let call = |method: reqwest::Method, path: String, body: Value| {
        let http = http.clone();
        let base = base.clone();
        let who = who.clone();
        async move {
            let res = http
                .request(method.clone(), format!("{base}{path}"))
                .header(llmtodo_api_types::CONTEXT_HEADER, who)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = res.status();
            let v: Value = res.json().await.unwrap_or(Value::Null);
            assert!(status.is_success(), "{method} {path} -> {status}: {v}");
            v
        }
    };
    call(
        reqwest::Method::POST,
        "/v1/teams".into(),
        json!({ "key": "LLM", "name": "llmtodo" }),
    )
    .await;

    use llmtodo_server::store::backend;
    let start = Instant::now();
    backend::query(
        "INSERT INTO issues (id, team_id, number, key, title, description, status_id, priority, created_at, updated_at, estimate_minutes, estimate_cost_usd)
         SELECT 'seed-' || n, t.id, n, 'LLM-' || n, 'Seeded issue ' || n, 'Text of issue ' || n,
                (SELECT s.id FROM statuses s WHERE s.team_id = t.id ORDER BY s.position LIMIT 1 OFFSET (n % 4)),
                n % 5, '2026-01-01T00:00:00.000000Z', '2026-01-01T00:00:00.000000Z', 10, 1.0
         FROM generate_series(1, ?) AS n, teams t",
    )
    .bind(issues)
    .execute(&direct)
    .await
    .unwrap();
    backend::query(
        "INSERT INTO relations (id, type, from_id, to_id, created_at)
         SELECT 'rel-' || n, 'blocks', 'seed-' || n, 'seed-' || (n + 1), '2026-01-01T00:00:00.000000Z'
         FROM generate_series(1, ? - 1) AS n WHERE n % 20 <> 0",
    )
    .bind(issues)
    .execute(&direct)
    .await
    .unwrap();
    // The first issue is a map with 200 sub-issues, for the frontier.
    backend::query("UPDATE issues SET parent_id = 'seed-1' WHERE number BETWEEN 2 AND 201")
        .execute(&direct)
        .await
        .unwrap();
    backend::query("INSERT INTO issue_keys (key, issue_id) SELECT key, id FROM issues")
        .execute(&direct)
        .await
        .unwrap();
    backend::query("UPDATE teams SET next_number = ? + 1")
        .bind(issues)
        .execute(&direct)
        .await
        .unwrap();
    backend::query("ANALYZE").execute(&direct).await.unwrap();
    eprintln!("seeded {issues} issues in {:?}", start.elapsed());
    direct.close().await;

    let mut out = Vec::new();
    let post = reqwest::Method::POST;
    let get = reqwest::Method::GET;
    // The app's first load: every open issue in pages of 1000, by ID.
    let snap = timed(&mut out, "snapshot: every open issue, by 5000", async {
        let mut cursor: Option<String> = None;
        let mut n = 0;
        loop {
            let page = call(
                post.clone(),
                "/v1/issues/query".into(),
                json!({ "order": "id", "filter": { "open": true }, "limit": 5000, "cursor": cursor }),
            )
            .await;
            n += page["items"].as_array().unwrap().len();
            match page["next_cursor"].as_str() {
                Some(c) => cursor = Some(c.to_string()),
                None => break,
            }
        }
        json!(n)
    })
    .await;
    eprintln!("snapshot rows: {snap}");
    let _ = timed(&mut out, "snapshot again, warm", async {
        let mut cursor: Option<String> = None;
        let mut n = 0;
        loop {
            let page = call(
                post.clone(),
                "/v1/issues/query".into(),
                json!({ "order": "id", "filter": { "open": true }, "limit": 5000, "cursor": cursor }),
            )
            .await;
            n += page["items"].as_array().unwrap().len();
            match page["next_cursor"].as_str() {
                Some(c) => cursor = Some(c.to_string()),
                None => break,
            }
        }
        json!(n)
    })
    .await;
    timed(&mut out, "issue query: first page of 100", async {
        call(
            post.clone(),
            "/v1/issues/query".into(),
            json!({ "limit": 100 }),
        )
        .await
    })
    .await;
    timed(&mut out, "issue query: text search", async {
        call(
            post.clone(),
            "/v1/issues/query".into(),
            json!({ "text": "seeded 4242" }),
        )
        .await
    })
    .await;
    let mid = issues / 2;
    timed(&mut out, "issue view", async {
        call(get.clone(), format!("/v1/issues/LLM-{mid}"), Value::Null).await
    })
    .await;
    timed(&mut out, "frontier of a map, 200 sub-issues", async {
        call(
            post.clone(),
            "/v1/issues/frontier".into(),
            json!({ "issue": "LLM-1" }),
        )
        .await
    })
    .await;
    // Each write runs twice. The first run prepares its statements on the
    // connections it gets, which costs a round trip each. A server that
    // has run a while has them prepared, as in the second run (LLM-469).
    for (round, issue) in [("", mid), (", warm", mid + 8)] {
        timed(&mut out, &format!("issue create{round}"), async {
            call(
                post.clone(),
                "/v1/issues".into(),
                json!({ "title": "New", "estimate_minutes": 5, "estimate_cost_usd": 1.0 }),
            )
            .await
        })
        .await;
        timed(&mut out, &format!("issue update: status{round}"), async {
            call(
                reqwest::Method::PATCH,
                format!("/v1/issues/LLM-{issue}"),
                json!({ "status": "In Progress" }),
            )
            .await
        })
        .await;
        timed(&mut out, &format!("issue claim{round}"), async {
            call(
                post.clone(),
                format!("/v1/issues/LLM-{}/claim", issue + 1),
                json!({}),
            )
            .await
        })
        .await;
    }

    eprintln!("\nround trip {:?}, {issues} issues", rtt);
    eprintln!(
        "{:<36} {:>10} {:>11} {:>10}",
        "request", "ms", "statements", "trips"
    );
    for t in &out {
        eprintln!(
            "{:<36} {:>10.0} {:>11} {:>10.1}",
            t.what,
            t.ms,
            t.statements,
            t.ms / rtt.as_secs_f64() / 1000.0
        );
    }
    running.stop().await.unwrap();
}
