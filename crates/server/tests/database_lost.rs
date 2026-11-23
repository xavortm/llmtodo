//! A server whose shared database goes away keeps answering, and says the
//! database is unreachable, with its host (LLM-470). A proxy between the
//! server and Postgres stands in for the network, and the test cuts it.
//! It runs only on Postgres:
//!
//! ```sh
//! docker compose up -d --wait postgres
//! LLMTODO_TEST_POSTGRES_URL=postgres://llmtodo:llmtodo@127.0.0.1:54329/llmtodo \
//!     cargo test -p llmtodo-server --test database_lost
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// A TCP proxy to `target`. Notifying the returned `Notify` closes every
/// connection and refuses new ones. Returns the proxy's port.
async fn cuttable_proxy(target: String) -> (u16, Arc<Notify>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let cut = Arc::new(Notify::new());
    let stop = cut.clone();
    tokio::spawn(async move {
        let mut links = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                _ = stop.notified() => {
                    // Dropping the listener refuses new connections, and
                    // aborting the links closes the open ones.
                    links.abort_all();
                    return;
                }
                accepted = listener.accept() => {
                    let Ok((mut client, _)) = accepted else { return };
                    let target = target.clone();
                    links.spawn(async move {
                        let Ok(mut server) = TcpStream::connect(&target).await else {
                            return;
                        };
                        let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
                    });
                }
            }
        }
    });
    (port, cut)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_database_is_named_and_the_server_keeps_answering() {
    let Some(admin) = std::env::var("LLMTODO_TEST_POSTGRES_URL")
        .ok()
        .filter(|u| !u.is_empty())
    else {
        eprintln!("skipped: set LLMTODO_TEST_POSTGRES_URL");
        return;
    };
    let url = llmtodo_server::db::create_test_database(&admin)
        .await
        .unwrap();
    let (_, target) = through(&url, 0);
    let (port, cut) = cuttable_proxy(target).await;
    let (proxied, _) = through(&url, port);
    let host = llmtodo_server::db::url_host(&proxied);

    let dir = tempfile::tempdir().unwrap();
    let running = llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.path().to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
        offline_copy: false,
        database: llmtodo_server::Database::Postgres { url: proxied },
    })
    .await
    .unwrap();
    let base = running.url();
    let http = reqwest::Client::new();
    let get = |path: &'static str| {
        let (http, base) = (http.clone(), base.clone());
        async move {
            let res = http.get(format!("{base}{path}")).send().await.unwrap();
            let status = res.status().as_u16();
            (status, res.json::<Value>().await.unwrap())
        }
    };

    let (status, health) = get("/v1/health").await;
    assert_eq!(status, 200);
    assert!(health["database"]["unreachable"].is_null(), "{health}");
    assert_eq!(get("/v1/teams").await.0, 200);

    cut.notify_one();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The health check still answers, soon, and says why.
    let start = Instant::now();
    let (status, health) = get("/v1/health").await;
    assert_eq!(status, 200, "{health}");
    assert!(
        health["database"]["unreachable"].is_string(),
        "the health check names the outage: {health}"
    );
    assert!(start.elapsed() < Duration::from_secs(5));

    // A request that needs the database fails with the new code, and the
    // message names the host but no password.
    let start = Instant::now();
    let (status, body) = get("/v1/teams").await;
    assert!(
        start.elapsed() < Duration::from_secs(15),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["error"]["code"], "database_unreachable", "{body}");
    assert_eq!(body["error"]["details"]["database_host"], host.as_str());
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with(&format!("Shared database unreachable at {host}")),
        "{message}"
    );
    assert!(!message.contains("llmtodo:llmtodo"), "{message}");
    running.stop().await.unwrap();
}
