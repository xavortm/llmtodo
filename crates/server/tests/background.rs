//! Write queueing and the daily backup, under load and after an error.

use std::time::Duration;

use llmtodo_api_types::ClientContext;
use llmtodo_server::state::{Ctx, Write};
use llmtodo_server::{Config, db, start, store};

async fn server(dir: &std::path::Path) -> llmtodo_server::Running {
    start(Config {
        data_dir: dir.to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap()
}

/// LLM-181: writers wait for the write lock without a pool connection, so a
/// burst of writes leaves connections for the readers.
#[tokio::test]
async fn waiting_writers_leave_connections_for_readers() {
    let dir = tempfile::tempdir().unwrap();
    let running = server(dir.path()).await;
    let st = running.state.clone();
    let actor = store::actors::owner(&st.pool()).await.unwrap();
    let ctx = Ctx {
        client: ClientContext::default(),
        actor: actor.clone(),
    };
    let first = Write::begin(&st, &ctx).await.unwrap();

    // More writers than the pool has connections.
    let mut writers = Vec::new();
    for _ in 0..12 {
        let st = st.clone();
        let actor = actor.clone();
        writers.push(tokio::spawn(async move {
            let ctx = Ctx {
                client: ClientContext::default(),
                actor,
            };
            Write::begin(&st, &ctx)
                .await
                .unwrap()
                .commit()
                .await
                .unwrap();
        }));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    let read = tokio::time::timeout(Duration::from_secs(2), async {
        let mut c = st.pool().acquire().await.unwrap();
        store::issues::count_all(&mut c).await.unwrap()
    })
    .await;
    assert_eq!(read.ok(), Some(0), "a read waits for no writer");

    first.commit().await.unwrap();
    for w in writers {
        tokio::time::timeout(Duration::from_secs(10), w)
            .await
            .expect("every queued write finishes")
            .unwrap();
    }
    running.stop().await.unwrap();
}

/// LLM-181: a copy that fails leaves no file, and a cut-short copy never
/// counts as the newest daily backup.
#[tokio::test]
async fn a_failed_backup_leaves_no_file_that_counts() {
    use std::os::unix::fs::PermissionsExt;

    // The pool alone, so the server's own backup loop does not race this test.
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();
    let backups = dir.path().join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    let names = || -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&backups)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };

    // A copy cut short by a crash keeps its temporary name.
    let cut = "llmtodo-daily-20261112T000000.000Z.db.partial";
    std::fs::write(backups.join(cut), b"x").unwrap();
    assert!(db::newest_daily_backup(dir.path()).is_none());

    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o555)).unwrap();
    let failed = db::backup(&pool, dir.path(), "daily").await;
    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(failed.is_err());
    assert_eq!(names(), [cut]);
    assert!(db::newest_daily_backup(dir.path()).is_none());

    let path = db::backup(&pool, dir.path(), "daily").await.unwrap();
    assert_eq!(path.extension().unwrap(), "db");
    assert!(db::newest_daily_backup(dir.path()).is_some());
    let whole = path.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(names(), [cut.to_string(), whole]);
    pool.close().await;
}
