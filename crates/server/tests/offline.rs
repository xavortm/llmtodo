//! Offline mode on a shared database (LLM-471). One server, A, reaches
//! Postgres through a proxy that the test cuts and restores, the way a
//! laptop loses and finds its network. A second server, B, stands for the
//! other laptop and stays online. They run only on Postgres:
//!
//! ```sh
//! docker compose up -d --wait postgres
//! LLMTODO_TEST_POSTGRES_URL=postgres://llmtodo:llmtodo@127.0.0.1:54329/llmtodo \
//!     cargo test -p llmtodo-server --test offline
//! ```

use std::path::Path;
use std::time::{Duration, Instant};

use base64::Engine;
use llmtodo_api_types::*;
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

fn admin_url() -> Option<String> {
    std::env::var("LLMTODO_TEST_POSTGRES_URL")
        .ok()
        .filter(|u| !u.is_empty())
}

/// A TCP proxy to `target` that can be cut and restored. While cut it
/// closes its open links and refuses new ones.
struct Proxy {
    port: u16,
    up: watch::Sender<bool>,
}

impl Proxy {
    async fn start(target: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (up, rx) = watch::channel(true);
        tokio::spawn(async move {
            loop {
                let Ok((mut client, _)) = listener.accept().await else {
                    return;
                };
                if !*rx.borrow() {
                    drop(client);
                    continue;
                }
                let target = target.clone();
                let mut rx = rx.clone();
                tokio::spawn(async move {
                    let Ok(mut server) = TcpStream::connect(&target).await else {
                        return;
                    };
                    tokio::select! {
                        _ = tokio::io::copy_bidirectional(&mut client, &mut server) => {}
                        _ = rx.wait_for(|up| !*up) => {}
                    }
                });
            }
        });
        Proxy { port, up }
    }

    fn cut(&self) {
        self.up.send_replace(false);
    }

    fn restore(&self) {
        self.up.send_replace(true);
    }
}

/// The URL with its host and port replaced by the proxy's, and the old
/// host and port.
fn through(url: &str, port: u16) -> (String, String) {
    let (scheme, rest) = url.split_once("://").unwrap();
    let (auth, hostpath) = rest.rsplit_once('@').unwrap();
    let (hostport, path) = hostpath.split_once('/').unwrap();
    (
        format!("{scheme}://{auth}@127.0.0.1:{port}/{path}"),
        hostport.to_string(),
    )
}

struct Server {
    url: String,
    http: reqwest::Client,
    running: llmtodo_server::Running,
    dir: tempfile::TempDir,
}

impl Server {
    async fn start(db_url: &str, host: &str, offline_copy: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let running = llmtodo_server::start(llmtodo_server::Config {
            data_dir: dir.path().to_path_buf(),
            port: 0,
            host: Some(host.into()),
            github_poll: false,
            offline_copy,
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
            dir,
        }
    }

    async fn call(&self, method: &str, path: &str, body: Value) -> (u16, Value) {
        let who = ClientContext {
            session_id: Some("owner-session".into()),
            ..Default::default()
        };
        let ctx = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&who).unwrap());
        let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut req = self
            .http
            .request(m, format!("{}{path}", self.url))
            .header(CONTEXT_HEADER, ctx);
        if method != "GET" && method != "DELETE" {
            req = req.json(&body);
        }
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        (status, res.json().await.unwrap_or(Value::Null))
    }

    async fn ok(&self, method: &str, path: &str, body: Value) -> Value {
        let (s, v) = self.call(method, path, body).await;
        assert!(s < 300, "{method} {path} -> {s}: {v}");
        v
    }

    async fn issue(&self, title: &str, description: &str) -> String {
        self.ok(
            "POST",
            "/v1/issues",
            json!({ "title": title, "description": description }),
        )
        .await["key"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// A fresh database has no team, and issues need one.
    async fn team(&self) {
        self.ok(
            "POST",
            "/v1/teams",
            json!({ "key": "LLM", "name": "llmtodo" }),
        )
        .await;
    }

    async fn health(&self) -> Value {
        self.ok("GET", "/v1/health", json!({})).await
    }

    /// Waits until the copy holds each issue as the shared database has it.
    async fn wait_for_copy(&self, keys: &[&str], titles: &[&str]) {
        let path = self.dir.path().join("mirror.db");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if has_issues(&path, keys, titles).await {
                return;
            }
            assert!(Instant::now() < deadline, "the copy never caught up");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Waits until the server is online with nothing waiting.
    async fn wait_online(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let h = self.health().await;
            let db = &h["database"];
            if db["offline"] == false && db["syncing"] == false && db["waiting"] == 0 {
                return h;
            }
            assert!(Instant::now() < deadline, "never came back online: {h}");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn stop(self) {
        self.running.stop().await.unwrap();
    }
}

/// True when the SQLite copy at `path` holds the issues with these titles.
async fn has_issues(path: &Path, keys: &[&str], titles: &[&str]) -> bool {
    use llmtodo_server::store::backend;
    if !path.exists() {
        return false;
    }
    let Ok(pool) = llmtodo_server::db::open_sqlite_file(path).await else {
        return false;
    };
    let mut ok = true;
    for (k, t) in keys.iter().zip(titles) {
        let title: Option<String> = backend::query_scalar(
            "SELECT i.title FROM issues i JOIN issue_keys k ON k.issue_id = i.id WHERE k.key = ?",
        )
        .bind(*k)
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten();
        ok &= title.as_deref() == Some(*t);
    }
    pool.close().await;
    ok
}

/// The first status of a type in the default team.
async fn status_of(s: &Server, kind: &str) -> String {
    let teams = s.ok("GET", "/v1/teams", json!({})).await;
    teams[0]["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|st| st["type"] == kind)
        .unwrap()["name"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_reads_work_writes_queue_and_replay_lands_them() {
    let Some(admin) = admin_url() else {
        eprintln!("skipped: set LLMTODO_TEST_POSTGRES_URL");
        return;
    };
    let db = llmtodo_server::db::create_test_database(&admin)
        .await
        .unwrap();
    let (_, target) = through(&db, 0);
    let proxy = Proxy::start(target).await;
    let (proxied, _) = through(&db, proxy.port);
    let a = Server::start(&proxied, "laptop-a", true).await;
    let b = Server::start(&db, "laptop-b", false).await;
    a.team().await;

    let x = a.issue("Before the cut", "Line one\n").await;
    a.wait_for_copy(&[&x], &["Before the cut"]).await;

    proxy.cut();
    // Reads work on the copy, and the server says it is offline.
    let view = a.ok("GET", &format!("/v1/issues/{x}"), json!({})).await;
    assert_eq!(view["title"], "Before the cut");
    let h = a.health().await;
    assert_eq!(h["database"]["offline"], true, "{h}");
    assert_eq!(h["database"]["backend"], "postgres", "{h}");
    assert!(h["database"]["unreachable"].is_null(), "{h}");

    // Writes queue.
    a.ok(
        "PATCH",
        &format!("/v1/issues/{x}"),
        json!({ "title": "Changed offline" }),
    )
    .await;
    let made = a
        .ok(
            "POST",
            "/v1/issues",
            json!({ "title": "Made offline", "description": "New\n" }),
        )
        .await;
    let provisional = made["key"].as_str().unwrap().to_string();
    assert!(provisional.contains("-~"), "{provisional}");
    let id = made["id"].as_str().unwrap().to_string();
    a.ok(
        "POST",
        &format!("/v1/issues/{provisional}/comments"),
        json!({ "body": "A comment offline" }),
    )
    .await;
    a.ok(
        "POST",
        &format!("/v1/issues/{x}/relations"),
        json!({ "type": "blocks", "issue": provisional }),
    )
    .await;
    a.ok(
        "PATCH",
        &format!("/v1/issues/{provisional}"),
        json!({ "parent": x }),
    )
    .await;
    let h = a.health().await;
    assert_eq!(h["database"]["waiting"], 5, "{h}");
    let outbox = a.ok("GET", "/v1/outbox", json!({})).await;
    assert_eq!(outbox["entries"].as_array().unwrap().len(), 5, "{outbox}");

    // A claim needs the shared database.
    let (status, body) = a
        .call(
            "POST",
            &format!("/v1/issues/{x}/claim"),
            json!({ "session_id": "s" }),
        )
        .await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["error"]["code"], "needs_network", "{body}");
    assert_eq!(
        body["error"]["message"],
        "Claims need the shared database. Try again when you are online."
    );
    assert_eq!(ErrorCode::NeedsNetwork.exit_code(), 12);
    // The other laptop sees none of it yet.
    assert_eq!(
        b.ok("GET", &format!("/v1/issues/{x}"), json!({})).await["title"],
        "Before the cut"
    );

    proxy.restore();
    a.wait_online().await;

    // Every change landed on the shared database.
    let on_b = b.ok("GET", &format!("/v1/issues/{x}"), json!({})).await;
    assert_eq!(on_b["title"], "Changed offline");
    let made_b = b.ok("GET", &format!("/v1/issues/{id}"), json!({})).await;
    let real = made_b["key"].as_str().unwrap();
    assert!(!real.contains('~'), "{real}");
    assert_eq!(made_b["parent"], x.as_str(), "{made_b}");
    assert_eq!(
        made_b["comments"][0]["body"], "A comment offline",
        "{made_b}"
    );
    // The provisional key resolves to the same issue after replay.
    let by_alias = b
        .ok("GET", &format!("/v1/issues/{provisional}"), json!({}))
        .await;
    assert_eq!(by_alias["id"], id.as_str());
    assert!(
        made_b["relations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["issue"]["key"] == x.as_str()),
        "{made_b}"
    );
    // And A reads it from Postgres again, with the real key.
    let on_a = a.ok("GET", &format!("/v1/issues/{id}"), json!({})).await;
    assert_eq!(on_a["key"], real);

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_conflict_rule_holds_on_replay() {
    let Some(admin) = admin_url() else {
        eprintln!("skipped: set LLMTODO_TEST_POSTGRES_URL");
        return;
    };
    let db = llmtodo_server::db::create_test_database(&admin)
        .await
        .unwrap();
    let (_, target) = through(&db, 0);
    let proxy = Proxy::start(target).await;
    let (proxied, _) = through(&db, proxy.port);
    let a = Server::start(&proxied, "laptop-a", true).await;
    let b = Server::start(&db, "laptop-b", false).await;
    a.team().await;
    let started = status_of(&b, "started").await;
    let done = status_of(&b, "completed").await;

    let base = "Line one\nLine two\nLine three\nLine four\nLine five\n";
    let fields = a.issue("Fields", "").await;
    let merged = a.issue("Merges", base).await;
    let clash = a.issue("Clashes", base).await;
    let trashed = a.issue("Trashed", "").await;
    let refused = a.issue("Refused", "").await;
    b.ok("POST", "/v1/labels", json!({ "name": "Doomed" }))
        .await;
    // A touch on A, so its copy refreshes the catalog with the label.
    a.ok("POST", "/v1/labels", json!({ "name": "Kept" })).await;
    a.wait_for_copy(
        &[&fields, &merged, &clash, &trashed, &refused],
        &["Fields", "Merges", "Clashes", "Trashed", "Refused"],
    )
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    proxy.cut();
    // B changes the status before A does, so A's later change wins.
    b.ok(
        "PATCH",
        &format!("/v1/issues/{fields}"),
        json!({ "status": started }),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    a.ok(
        "PATCH",
        &format!("/v1/issues/{fields}"),
        json!({ "status": done, "priority": "high", "title": "Fields offline" }),
    )
    .await;
    a.ok(
        "POST",
        &format!("/v1/issues/{fields}/comments"),
        json!({ "body": "Comments always apply" }),
    )
    .await;
    a.ok(
        "PATCH",
        &format!("/v1/issues/{merged}"),
        json!({ "body": base.replace("Line one", "Line ONE (offline)"), "force": true }),
    )
    .await;
    a.ok(
        "PATCH",
        &format!("/v1/issues/{clash}"),
        json!({ "body": base.replace("Line three", "Line three, offline"), "force": true }),
    )
    .await;
    a.ok(
        "PATCH",
        &format!("/v1/issues/{trashed}"),
        json!({ "title": "Edited while trashed" }),
    )
    .await;
    a.ok(
        "PATCH",
        &format!("/v1/issues/{refused}"),
        json!({ "add_labels": ["Doomed"] }),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    // B changes the priority after A does, so B's change wins.
    b.ok(
        "PATCH",
        &format!("/v1/issues/{fields}"),
        json!({ "priority": "low" }),
    )
    .await;
    b.ok(
        "PATCH",
        &format!("/v1/issues/{merged}"),
        json!({ "body": base.replace("Line five", "Line FIVE (online)"), "force": true }),
    )
    .await;
    b.ok(
        "PATCH",
        &format!("/v1/issues/{clash}"),
        json!({ "body": base.replace("Line three", "Line three, online"), "force": true }),
    )
    .await;
    b.ok("DELETE", &format!("/v1/issues/{trashed}"), json!({}))
        .await;
    b.ok("DELETE", "/v1/labels/Doomed", json!({})).await;

    proxy.restore();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let h = a.health().await;
        if h["database"]["offline"] == false && h["database"]["syncing"] == false {
            assert_eq!(h["database"]["waiting"], 0, "{h}");
            assert_eq!(h["database"]["failed"], 1, "{h}");
            break;
        }
        assert!(Instant::now() < deadline, "never came back online: {h}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Fields: the later change wins, each one by its own time.
    let f = b
        .ok("GET", &format!("/v1/issues/{fields}"), json!({}))
        .await;
    assert_eq!(f["status"]["name"], done.as_str(), "A's later status: {f}");
    assert_eq!(f["priority"], "low", "B's later priority: {f}");
    assert_eq!(f["title"], "Fields offline", "nobody else changed it: {f}");
    assert!(
        f["comments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["body"] == "Comments always apply"),
        "{f}"
    );
    let history = b
        .ok("GET", &format!("/v1/issues/{fields}/history"), json!({}))
        .await;
    let lost: Vec<&Value> = history["activity"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["field"] == "sync_conflict")
        .collect();
    assert_eq!(lost.len(), 1, "{history}");
    assert_eq!(lost[0]["new"]["priority"]["priority"], "high", "{history}");

    // A description both changed in different places merges.
    let m = b
        .ok("GET", &format!("/v1/issues/{merged}"), json!({}))
        .await;
    assert_eq!(
        m["description"],
        "Line ONE (offline)\nLine two\nLine three\nLine four\nLine FIVE (online)\n"
    );

    // The same lines clash: the shared text stays, the offline text goes
    // into a conflict section, and the issue waits on the owner.
    let c = b.ok("GET", &format!("/v1/issues/{clash}"), json!({})).await;
    let text = c["description"].as_str().unwrap();
    assert!(
        text.starts_with(&base.replace("Line three", "Line three, online")),
        "{text}"
    );
    assert!(text.contains("+++ Sync conflict\n"), "{text}");
    assert!(text.contains("Line three, offline"), "{text}");
    assert_eq!(c["waiting_on"], "owner", "{c}");

    // An issue trashed meanwhile keeps the edit in the trash.
    let t = b
        .ok("GET", &format!("/v1/issues/{trashed}"), json!({}))
        .await;
    assert_eq!(t["title"], "Edited while trashed", "{t}");
    assert!(t["deleted_at"].is_string(), "{t}");

    // A change the shared database refuses stays in the outbox, failed.
    let outbox = a.ok("GET", "/v1/outbox", json!({})).await;
    let entries = outbox["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{outbox}");
    assert_eq!(entries[0]["state"], "failed");
    assert_eq!(entries[0]["error"]["code"], "not_found", "{outbox}");
    let row = entries[0]["id"].as_i64().unwrap();
    let after = a
        .ok("DELETE", &format!("/v1/outbox/{row}"), json!({}))
        .await;
    assert!(after["entries"].as_array().unwrap().is_empty(), "{after}");
    assert_eq!(a.health().await["database"]["failed"], 0);

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_with_a_copy_starts_offline_and_replays_later() {
    let Some(admin) = admin_url() else {
        eprintln!("skipped: set LLMTODO_TEST_POSTGRES_URL");
        return;
    };
    let db = llmtodo_server::db::create_test_database(&admin)
        .await
        .unwrap();
    let (_, target) = through(&db, 0);
    let proxy = Proxy::start(target).await;
    let (proxied, _) = through(&db, proxy.port);
    let dir = tempfile::tempdir().unwrap();
    let start = |dir: &Path| {
        let dir = dir.to_path_buf();
        let url = proxied.clone();
        async move {
            llmtodo_server::start(llmtodo_server::Config {
                data_dir: dir,
                port: 0,
                host: Some("laptop-a".into()),
                github_poll: false,
                offline_copy: true,
                database: llmtodo_server::Database::Postgres { url },
            })
            .await
        }
    };

    // No copy and no network: the server does not start, as before.
    proxy.cut();
    let err = start(dir.path()).await.err().expect("no start");
    assert!(
        err.downcast_ref::<llmtodo_server::db::Unreachable>()
            .is_some(),
        "{err:#}"
    );

    proxy.restore();
    let running = start(dir.path()).await.unwrap();
    let a = Server {
        url: running.url(),
        http: reqwest::Client::new(),
        running,
        dir,
    };
    a.team().await;
    let x = a.issue("Kept", "").await;
    a.wait_for_copy(&[&x], &["Kept"]).await;
    let dir = a.dir;
    a.running.stop().await.unwrap();

    // With a copy and no network, it starts offline and queues writes.
    proxy.cut();
    let running = start(dir.path()).await.unwrap();
    let a = Server {
        url: running.url(),
        http: reqwest::Client::new(),
        running,
        dir,
    };
    assert_eq!(a.health().await["database"]["offline"], true);
    a.ok(
        "PATCH",
        &format!("/v1/issues/{x}"),
        json!({ "title": "Changed while started offline" }),
    )
    .await;
    proxy.restore();
    a.wait_online().await;
    let b = Server::start(&db, "laptop-b", false).await;
    assert_eq!(
        b.ok("GET", &format!("/v1/issues/{x}"), json!({})).await["title"],
        "Changed while started offline"
    );
    a.stop().await;
    b.stop().await;
}
