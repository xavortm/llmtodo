//! A server that accepts the connection but never answers must not hang the
//! client. The request fails as unreachable, which the CLI maps to exit 7.

use std::time::{Duration, Instant};

use llmtodo_client::{Client, ClientError};

#[tokio::test]
async fn silent_server_times_out_as_unreachable() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // Accept connections and hold them open without a reply, like a server
    // stuck on a SQLite lock.
    let hold = tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            held.push(socket);
        }
    });

    let client = Client::new(format!("http://127.0.0.1:{port}"), &Default::default())
        .with_timeouts(Duration::from_millis(300), Duration::from_millis(600));
    let started = Instant::now();
    let err = tokio::time::timeout(Duration::from_secs(5), client.health())
        .await
        .expect("the request hung past its timeout")
        .expect_err("a silent server cannot answer");
    assert!(matches!(err, ClientError::Unreachable(_)), "{err:?}");
    assert_eq!(err.exit_code(), 7);
    assert!(err.to_string().contains("did not answer"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(2));

    // Long routes use the long limit, and still end.
    let err = tokio::time::timeout(Duration::from_secs(5), client.github_sync())
        .await
        .expect("the long request hung past its timeout")
        .expect_err("a silent server cannot answer");
    assert_eq!(err.exit_code(), 7);
    hold.abort();
}
