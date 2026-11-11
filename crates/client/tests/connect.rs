//! `Connect` must not trust a stale `server.json` that points at a server for
//! another data folder.

use llmtodo_api_types::ServerInfo;
use llmtodo_client::{ClientError, Connect};
use std::path::Path;

async fn start(dir: &Path) -> llmtodo_server::Running {
    llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap()
}

fn connect(dir: &Path) -> Connect {
    Connect {
        data_dir: dir.to_path_buf(),
        context: Default::default(),
        spawn: None,
        upgrade_old_server: false,
    }
}

fn write_info(dir: &Path, info: &ServerInfo) {
    std::fs::write(dir.join("server.json"), serde_json::to_vec(info).unwrap()).unwrap();
}

/// Server A crashed and left `server.json`. Server C later took the same
/// port. A client for A must not talk to C.
#[tokio::test]
async fn rejects_server_for_another_data_folder() {
    let a = tempfile::tempdir().unwrap();
    let c_dir = tempfile::tempdir().unwrap();
    let c = start(c_dir.path()).await;
    let c_info: ServerInfo =
        serde_json::from_slice(&std::fs::read(c_dir.path().join("server.json")).unwrap()).unwrap();
    // A stale file in A that names C's port and PID.
    write_info(a.path(), &c_info);

    let err = connect(a.path())
        .connect()
        .await
        .err()
        .expect("connected to the wrong server");
    assert!(matches!(err, ClientError::Unreachable(_)), "{err:?}");
    // C's own folder still connects.
    assert!(connect(c_dir.path()).connect().await.is_ok());
    c.stop().await.unwrap();
}

/// A server for the right folder, but `server.json` names another PID, so the
/// file is not from this server.
#[tokio::test]
async fn rejects_server_json_with_other_pid() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(dir.path()).await;
    let mut info: ServerInfo =
        serde_json::from_slice(&std::fs::read(dir.path().join("server.json")).unwrap()).unwrap();
    info.pid = info.pid.wrapping_add(1);
    write_info(dir.path(), &info);
    assert!(connect(dir.path()).connect().await.is_err());
    s.stop().await.unwrap();
}
