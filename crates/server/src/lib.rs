//! The llmtodo API server: axum over one SQLite file.
//!
//! The `llmtodo` binary runs it with `llmtodo server run`. Tests start it in
//! process with [`start`] on a free port.

/// A query string built only from constants inside this crate, never from
/// input. Values always go through `bind`.
#[macro_export]
macro_rules! sql {
    ($($t:tt)*) => { $crate::store::backend::Sql::assert_safe(format!($($t)*)) };
}

pub mod db;
pub mod delta;
pub mod error;
pub mod github;
pub mod import;
pub mod merge;
pub mod offline;
pub mod ops;
pub mod routes;
pub mod state;
pub mod store;

use std::fs::File;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::Context;
use jiff::Timestamp;
use llmtodo_api_types::{DEFAULT_PORT, ServerInfo, VERSION};
use tokio::sync::{Notify, broadcast};

use crate::store::backend::Backend;

use crate::state::{AppState, Inner};

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    /// 0 picks a free port. `server.json` records the real one.
    pub port: u16,
    /// This machine's name for sync. Defaults to `LLMTODO_HOST`, then the
    /// short hostname.
    pub host: Option<String>,
    /// Poll GitHub in the background. Off in tests.
    pub github_poll: bool,
    /// Where the data lives: the SQLite file in the data folder, or a shared
    /// Postgres database (LLM-450).
    pub database: Database,
    /// On a shared database, keep a local copy and work on it while the
    /// database is out of reach (LLM-471). `llmtodo server run` turns it
    /// on. Tests leave it off unless they test offline mode.
    pub offline_copy: bool,
}

impl Config {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            port: DEFAULT_PORT,
            host: None,
            github_poll: true,
            database: Database::Sqlite,
            offline_copy: true,
        }
    }
}

/// The database a server opens.
#[derive(Clone, Default)]
pub enum Database {
    /// `llmtodo.db` in the data folder. The default.
    #[default]
    Sqlite,
    /// A shared Postgres database. The URL holds the password, so it never
    /// goes in a log or an error message.
    Postgres { url: String },
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Database::Sqlite => f.write_str("Sqlite"),
            Database::Postgres { url } => write!(f, "Postgres({})", db::redact_url(url)),
        }
    }
}

/// The database for a test's data folder: a Postgres database of its own
/// when `LLMTODO_TEST_POSTGRES_URL` is set, else SQLite. The folder keeps
/// the database's URL, so a test that restarts its server opens the same
/// data, and tests running at once never share it.
#[doc(hidden)]
pub async fn test_database(data_dir: &Path) -> Database {
    match std::env::var("LLMTODO_TEST_POSTGRES_URL") {
        Ok(admin) if !admin.is_empty() => {
            let marker = data_dir.join("test-postgres-url");
            let url = match std::fs::read_to_string(&marker) {
                Ok(url) => url,
                Err(_) => {
                    let url = db::create_test_database(&admin)
                        .await
                        .expect("create a test database on LLMTODO_TEST_POSTGRES_URL");
                    std::fs::create_dir_all(data_dir).ok();
                    std::fs::write(&marker, &url).expect("write the test database marker");
                    url
                }
            };
            Database::Postgres { url }
        }
        _ => Database::Sqlite,
    }
}

/// True when tests run against Postgres. Tests that open the SQLite file
/// themselves skip then.
#[doc(hidden)]
pub fn testing_postgres() -> bool {
    std::env::var("LLMTODO_TEST_POSTGRES_URL").is_ok_and(|u| !u.is_empty())
}

/// The data folder of the bare server binary: `LLMTODO_DATA_DIR`, else
/// the standard folder. Clients choose with `llmtodo_client::data_dir`,
/// which also reads the global config, and pass the folder in.
pub fn default_data_dir() -> PathBuf {
    std::env::var_os(llmtodo_core::data_dir::DATA_DIR_ENV)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(llmtodo_core::data_dir::standard_data_dir)
}

pub fn default_host() -> String {
    if let Some(h) = std::env::var("LLMTODO_HOST").ok().filter(|h| !h.is_empty()) {
        return h;
    }
    std::process::Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".into())
}

pub fn server_json_path(data_dir: &Path) -> PathBuf {
    data_dir.join("server.json")
}

/// A running server. Dropping it does not stop it; call [`Running::stop`].
pub struct Running {
    pub addr: SocketAddr,
    pub state: AppState,
    handle: tokio::task::JoinHandle<anyhow::Result<()>>,
    _lock: File,
}

impl Running {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.addr.port())
    }

    /// Starts recording the route of every request, for tests.
    pub fn record_routes(&self) {
        *self.state.route_log.lock().unwrap() = Some(Vec::new());
    }

    /// The routes seen since the last call, as `METHOD /path/{param}`.
    pub fn take_routes(&self) -> Vec<String> {
        self.state
            .route_log
            .lock()
            .unwrap()
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    pub async fn stop(self) -> anyhow::Result<()> {
        self.state.shutdown.notify_waiters();
        self.handle.await.context("server task panicked")?
    }

    /// Waits until the server stops by itself, such as after `/v1/server/stop`.
    pub async fn wait(self) -> anyhow::Result<()> {
        self.handle.await.context("server task panicked")?
    }
}

/// Opens the database, takes the server lock, binds, and serves in a
/// background task.
pub async fn start(config: Config) -> anyhow::Result<Running> {
    let data_dir = config.data_dir.clone();
    std::fs::create_dir_all(&data_dir).with_context(|| format!("create {}", data_dir.display()))?;
    let lock_path = data_dir.join("server.lock");
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("open {}", lock_path.display()))?;
    if lock.try_lock().is_err() {
        anyhow::bail!(
            "another llmtodo server already runs on {}; see {}",
            data_dir.display(),
            server_json_path(&data_dir).display()
        );
    }

    let (pool, shared) = match &config.database {
        Database::Sqlite => (db::open(&data_dir).await?, None),
        Database::Postgres { url } if config.offline_copy => {
            let s = offline::start(url, &data_dir).await?;
            (s.pool, Some(Arc::new(s.shared)))
        }
        Database::Postgres { url } => (db::open_postgres(url, Some(&data_dir)).await?, None),
    };
    store::actors::seed(&pool).await?;
    // A shared database has no primary: every server on it writes. The
    // flag belongs to laptop handoff, which only SQLite has.
    let is_primary = pool.backend() == Backend::Postgres
        || shared.is_some()
        || db::get_setting(&pool, "sync_is_primary")
            .await?
            .is_none_or(|v| v != "0");
    let (events, _) = broadcast::channel(1024);
    let sent_seq = store::activity::max_seq(&pool)
        .await
        .map_err(|e| anyhow::anyhow!(e.message))?;
    let state = AppState(Arc::new(Inner {
        pool_slot: std::sync::RwLock::new(pool),
        host: config.host.clone().unwrap_or_else(default_host),
        database_host: match &config.database {
            Database::Sqlite => None,
            Database::Postgres { url } => Some(db::url_host(url)),
        },
        data_dir: data_dir.clone(),
        events,
        sent_seq: tokio::sync::Mutex::new(sent_seq),
        started_at: Timestamp::now(),
        shutdown: Notify::new(),
        is_primary: AtomicBool::new(is_primary),
        writes: Default::default(),
        write_queue: Default::default(),
        route_log: Default::default(),
        last_request: std::sync::atomic::AtomicI64::new(Timestamp::now().as_second()),
        actor_cache: Default::default(),
        shared,
        router: std::sync::OnceLock::new(),
    }));
    let _ = state.router.set(routes::router);

    // Offline, the copy takes no writes outside the outbox.
    if !state.is_offline() {
        ops::claims::sweep_review_claims(&state)
            .await
            .map_err(|e| anyhow::anyhow!("end claims on review tickets: {}", e.message))?;
    }

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", config.port))
        .await
        .with_context(|| format!("bind 127.0.0.1:{}", config.port))?;
    let addr = listener.local_addr()?;
    let info = ServerInfo {
        port: addr.port(),
        pid: std::process::id(),
        version: VERSION.to_string(),
        started_at: state.started_at,
    };
    std::fs::write(
        server_json_path(&data_dir),
        serde_json::to_vec_pretty(&info)?,
    )?;
    tracing::info!(%addr, data_dir = %data_dir.display(), "llmtodo server listening");

    let app = routes::router(state.clone());
    let st = state.clone();
    tokio::spawn(backup_loop(state.clone()));
    if state.is_shared() {
        tokio::spawn(db::listen_loop(state.clone()));
    }
    offline::spawn(&state);
    if config.github_poll {
        tokio::spawn(github::poll_loop(state.clone()));
    }
    let handle = tokio::spawn(async move {
        let shutdown = st.clone();
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { shutdown.shutdown.notified().await })
            .await
            .context("serve")?;
        // Leave server.json only if it is still ours.
        let path = server_json_path(&st.data_dir);
        if let Ok(raw) = std::fs::read(&path)
            && serde_json::from_slice::<ServerInfo>(&raw)
                .is_ok_and(|i| i.pid == std::process::id() && i.port == addr.port())
        {
            std::fs::remove_file(&path).ok();
        }
        st.pool().close().await;
        if let Some(shared) = &st.shared {
            if let Some(p) = shared.pg() {
                p.close().await;
            }
            if let Some(m) = shared.mirror() {
                m.close().await;
            }
        }
        tracing::info!("llmtodo server stopped");
        Ok(())
    });
    Ok(Running {
        addr,
        state,
        handle,
        _lock: lock,
    })
}

/// How long after start the first daily backup waits. `VACUUM INTO` copies
/// the whole database, so it waits out the app's first load (LLM-342).
const FIRST_BACKUP_DELAY: Duration = Duration::from_secs(60);

/// The wait before the next daily backup. One that is due at start still
/// waits `FIRST_BACKUP_DELAY`.
fn backup_wait(due: Duration, first: bool) -> Duration {
    if first {
        due.max(FIRST_BACKUP_DELAY)
    } else {
        due
    }
}

/// Writes a daily backup, and one soon after startup when the newest is older
/// than a day, because the server may not run every day.
async fn backup_loop(state: AppState) {
    let day = Duration::from_secs(24 * 3600);
    let mut failures = 0u32;
    let mut first = true;
    loop {
        let due = match db::newest_daily_backup(&state.data_dir) {
            None => Duration::ZERO,
            Some(t) => day.saturating_sub(t.elapsed().unwrap_or(day)),
        };
        tokio::select! {
            _ = tokio::time::sleep(backup_wait(due, first)) => {}
            _ = state.shutdown.notified() => return,
        }
        first = false;
        // Offline, the pool is the local copy, which is no backup.
        if state.is_offline() {
            continue;
        }
        let has_data = match state.pool().acquire().await {
            Ok(mut c) => store::issues::count_all(&mut c).await.unwrap_or(0),
            Err(_) => 0,
        };
        if has_data > 0 || due.is_zero() {
            match db::backup(&state.pool(), &state.data_dir, "daily").await {
                Ok(_) => failures = 0,
                Err(e) => {
                    failures = failures.saturating_add(1);
                    tracing::warn!(error = %e, failures, "daily backup failed");
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(backup_retry_delay(failures)) => {}
            _ = state.shutdown.notified() => return,
        }
    }
}

/// The pause before the backup loop looks again. A failed backup copies the
/// whole database, so each failure in a row doubles the pause, from 2 minutes
/// up to 6 hours.
fn backup_retry_delay(failures: u32) -> Duration {
    let minute = Duration::from_secs(60);
    if failures == 0 {
        return minute;
    }
    let cap = Duration::from_secs(6 * 3600);
    minute.saturating_mul(1u32 << failures.min(16)).min(cap)
}

/// Logs to stderr and to daily files in `logs/` of the data folder.
pub fn init_logging(data_dir: &Path) -> tracing_appender::non_blocking::WorkerGuard {
    use tracing_subscriber::prelude::*;
    let file = tracing_appender::rolling::daily(data_dir.join("logs"), "server.log");
    let (writer, guard) = tracing_appender::non_blocking(file);
    let filter = tracing_subscriber::EnvFilter::try_from_env("LLMTODO_LOG")
        .unwrap_or_else(|_| "info,sqlx=warn".into());
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer),
        )
        .init();
    guard
}

/// Runs until stopped by `/v1/server/stop` or Ctrl-C.
pub async fn run(config: Config) -> anyhow::Result<()> {
    let running = start(config).await?;
    let state = running.state.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            state.shutdown.notify_waiters();
        }
    });
    running.wait().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_backup_waits_a_minute_after_start() {
        assert_eq!(backup_wait(Duration::ZERO, true), FIRST_BACKUP_DELAY);
        assert_eq!(backup_wait(Duration::ZERO, false), Duration::ZERO);
        let later = Duration::from_secs(3600);
        assert_eq!(backup_wait(later, true), later);
    }

    #[test]
    fn a_failed_backup_backs_off_up_to_six_hours() {
        let mins = |f| backup_retry_delay(f).as_secs() / 60;
        assert_eq!(
            mins(0),
            1,
            "after a success the loop looks again each minute"
        );
        assert_eq!(mins(1), 2);
        assert_eq!(mins(2), 4);
        assert_eq!(mins(5), 32);
        assert_eq!(mins(9), 360);
        assert_eq!(mins(u32::MAX), 360);
    }
}
