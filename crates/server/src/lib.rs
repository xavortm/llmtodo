//! The llmtodo API server: axum over one SQLite file.
//!
//! The `llmtodo` binary runs it with `llmtodo server run`. Tests start it in
//! process with [`start`] on a free port.

/// A query string built only from constants inside this crate, never from
/// input. Values always go through `bind`.
#[macro_export]
macro_rules! sql {
    ($($t:tt)*) => { sqlx::AssertSqlSafe(format!($($t)*)) };
}

pub mod db;
pub mod error;
pub mod github;
pub mod import;
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
}

impl Config {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            port: DEFAULT_PORT,
            host: None,
            github_poll: true,
        }
    }
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

    let pool = db::open(&data_dir).await?;
    store::actors::seed(&pool).await?;
    let is_primary = db::get_setting(&pool, "sync_is_primary")
        .await?
        .is_none_or(|v| v != "0");
    let (events, _) = broadcast::channel(1024);
    let sent_seq = store::activity::max_seq(&pool)
        .await
        .map_err(|e| anyhow::anyhow!(e.message))?;
    let state = AppState(Arc::new(Inner {
        pool_slot: std::sync::RwLock::new(pool),
        host: config.host.clone().unwrap_or_else(default_host),
        data_dir: data_dir.clone(),
        events,
        sent_seq: tokio::sync::Mutex::new(sent_seq),
        started_at: Timestamp::now(),
        shutdown: Notify::new(),
        is_primary: AtomicBool::new(is_primary),
        writes: Default::default(),
        write_queue: Default::default(),
        route_log: Default::default(),
    }));

    ops::claims::sweep_review_claims(&state)
        .await
        .map_err(|e| anyhow::anyhow!("end claims on review tickets: {}", e.message))?;

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

/// Writes a daily backup, and one at startup when the newest is older than a
/// day, because the server may not run every day.
async fn backup_loop(state: AppState) {
    let day = Duration::from_secs(24 * 3600);
    let mut failures = 0u32;
    loop {
        let due = match db::newest_daily_backup(&state.data_dir) {
            None => Duration::ZERO,
            Some(t) => day.saturating_sub(t.elapsed().unwrap_or(day)),
        };
        tokio::select! {
            _ = tokio::time::sleep(due) => {}
            _ = state.shutdown.notified() => return,
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
