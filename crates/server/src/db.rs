//! Opening the database, migrations, backups, IDs and timestamps.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::Context;
use jiff::Timestamp;
use sqlx::sqlite::SqliteConnection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::store::backend;
use crate::store::backend::Pool;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// The Postgres schema. Its first migration matches SQLite's 0020, and each
/// later change needs a migration in both folders.
pub static PG_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations-postgres");

/// A Postgres URL with the password replaced, for logs and errors.
pub fn redact_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return "<invalid URL>".into();
    };
    let Some((userinfo, host)) = rest.rsplit_once('@') else {
        return url.to_string();
    };
    let user = userinfo.split(':').next().unwrap_or("");
    format!("{scheme}://{user}:***@{host}")
}

/// The host, port and database name of a Postgres URL, with no user,
/// password or options.
pub fn url_host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
    rest.split('?').next().unwrap_or(rest).to_string()
}

/// Connects to a shared Postgres database and runs pending migrations.
///
/// With a data folder, a backup goes there before a migration of a database
/// that already has a schema, as on SQLite.
pub async fn open_postgres(url: &str, data_dir: Option<&Path>) -> anyhow::Result<Pool> {
    use sqlx::postgres::PgPoolOptions;
    let shown = redact_url(url);
    let pg = PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(30))
        .connect(url)
        .await
        .map_err(|e| anyhow::anyhow!("connect to Postgres at {shown}: {}", pg_error(&e)))?;
    let pool = Pool::Postgres(pg.clone());
    let newest_known = PG_MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0);
    let has_table: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(&pg)
        .await?;
    if has_table {
        let newest_applied: i64 =
            sqlx::query_scalar("SELECT coalesce(max(version), 0) FROM _sqlx_migrations")
                .fetch_one(&pg)
                .await?;
        if newest_applied > newest_known {
            anyhow::bail!(
                "the Postgres schema {newest_applied} is newer than this build supports ({newest_known}); upgrade llmtodo"
            );
        }
        if let Some(dir) = data_dir
            && newest_applied > 0
            && newest_applied < newest_known
        {
            backup(&pool, dir, "pre-migrate").await?;
        }
    }
    PG_MIGRATOR
        .run(&pg)
        .await
        .context("run the Postgres migrations")?;
    crate::ops::questions::backfill(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("fill the questions table: {}", e.message))?;
    crate::store::revisions::compact(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("store old revisions as deltas: {}", e.message))?;
    Ok(pool)
}

/// A connection error without the URL, which sqlx can include.
fn pg_error(e: &sqlx::Error) -> String {
    match e {
        sqlx::Error::Database(d) => d.message().to_string(),
        sqlx::Error::Io(e) => e.to_string(),
        sqlx::Error::Tls(e) => format!("TLS: {e}"),
        sqlx::Error::PoolTimedOut => "timed out".into(),
        // sqlx can quote the URL in a configuration error, so the message
        // stays generic.
        sqlx::Error::Configuration(_) => "the connection string is not a valid Postgres URL".into(),
        other => other.to_string(),
    }
}

/// Creates a fresh database next to the one in `admin_url`, and returns
/// its URL. For tests.
pub async fn create_test_database(admin_url: &str) -> anyhow::Result<String> {
    use sqlx::Connection as _;
    let name = format!("llmtodo_test_{}", uuid::Uuid::now_v7().simple());
    let mut c = sqlx::PgConnection::connect(admin_url).await?;
    // Test databases are never dropped by their tests, which may panic. The
    // name holds a UUIDv7, so each run drops the ones over 30 minutes old.
    let old: Vec<String> = sqlx::query_scalar(
        "SELECT datname::text FROM pg_database WHERE datname LIKE 'llmtodo\\_test\\_%'",
    )
    .fetch_all(&mut c)
    .await?;
    let cutoff = Timestamp::now() - jiff::SignedDuration::from_mins(30);
    for db in old {
        let made = uuid::Uuid::try_parse(db.trim_start_matches("llmtodo_test_"))
            .ok()
            .and_then(|u| u.get_timestamp())
            .and_then(|t| {
                let (secs, nanos) = t.to_unix();
                Timestamp::new(secs as i64, nanos as i32).ok()
            });
        if made.is_some_and(|m| m < cutoff) {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "DROP DATABASE IF EXISTS {db} WITH (FORCE)"
            )))
            .execute(&mut c)
            .await
            .ok();
        }
    }
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&mut c)
        .await?;
    c.close().await?;
    let (base, query) = match admin_url.split_once('?') {
        Some((b, q)) => (b, format!("?{q}")),
        None => (admin_url, String::new()),
    };
    let (prefix, _) = base
        .rsplit_once('/')
        .context("the test URL needs a database name")?;
    Ok(format!("{prefix}/{name}{query}"))
}

/// Days of daily backups to keep, and copies of each event backup kind.
pub const KEEP_BACKUPS: usize = 14;

pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Fixed-width UTC text, so text order is time order.
pub fn ts(t: Timestamp) -> String {
    t.strftime("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}

pub fn now() -> String {
    ts(Timestamp::now())
}

pub fn parse_ts(s: &str) -> Timestamp {
    Timestamp::from_str(s).unwrap_or(Timestamp::UNIX_EPOCH)
}

pub fn parse_ts_opt(s: Option<String>) -> Option<Timestamp> {
    s.as_deref().map(parse_ts)
}

/// Reads a stored ID as a UUID or as one of the ID types in core.
pub fn parse_uuid<T: From<uuid::Uuid>>(s: &str) -> T {
    T::from(uuid::Uuid::parse_str(s).unwrap_or_default())
}

pub fn db_path(data_dir: &Path) -> PathBuf {
    data_dir.join("llmtodo.db")
}

fn options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(15))
        // 32 MB page cache per connection and 256 MB of memory-mapped reads,
        // so repeated list and search reads skip the read syscalls (LLM-342).
        .pragma("cache_size", "-32000")
        .pragma("mmap_size", "268435456")
}

/// Opens the pool and runs pending migrations. A backup is written before any
/// migration of an existing database.
pub async fn open(data_dir: &Path) -> anyhow::Result<Pool> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("create data folder {}", data_dir.display()))?;
    let path = db_path(data_dir);
    let existed = path.exists();
    let sqlite = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options(&path))
        .await
        .with_context(|| format!("open database {}", path.display()))?;
    let pool = Pool::Sqlite(sqlite.clone());

    if existed {
        let applied: i64 =
            backend::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
                .fetch_one(&pool)
                .await
                .unwrap_or(0);
        let pending = MIGRATOR
            .iter()
            .filter(|m| m.migration_type.is_up_migration())
            .count() as i64
            > applied;
        if pending && applied > 0 {
            backup(&pool, data_dir, "pre-migrate").await?;
        }
        let newest_known = MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0);
        let newest_applied: i64 =
            backend::query_scalar("SELECT coalesce(max(version), 0) FROM _sqlx_migrations")
                .fetch_one(&pool)
                .await
                .unwrap_or(0);
        if newest_applied > newest_known {
            anyhow::bail!(
                "database schema {newest_applied} is newer than this build supports ({newest_known}); upgrade llmtodo"
            );
        }
    }
    MIGRATOR.run(&sqlite).await.context("run migrations")?;
    crate::ops::questions::backfill(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("fill the questions table: {}", e.message))?;
    crate::store::revisions::compact(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("store old revisions as deltas: {}", e.message))?;
    Ok(pool)
}

pub async fn schema_version(pool: &Pool) -> i64 {
    // Both migrators record their versions in _sqlx_migrations.
    backend::query_scalar("SELECT coalesce(max(version), 0) FROM _sqlx_migrations WHERE success")
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

/// Writes a consistent copy with `VACUUM INTO` and trims old copies of the
/// same kind.
pub async fn backup(pool: &Pool, data_dir: &Path, reason: &str) -> anyhow::Result<PathBuf> {
    let dir = data_dir.join("backups");
    std::fs::create_dir_all(&dir)?;
    let stamp = Timestamp::now().strftime("%Y%m%dT%H%M%S%.3fZ");
    let path = dir.join(format!("llmtodo-{reason}-{stamp}.db"));
    // The copy gets its real name only once it is whole. A copy cut short by
    // a crash keeps the `.partial` name, so it never counts as a backup.
    let partial = path.with_extension("db.partial");
    match pool {
        Pool::Sqlite(_) => vacuum_into(pool, &partial).await?,
        Pool::Postgres(_) => {
            if let Err(e) = export_to_sqlite(pool, &partial).await {
                std::fs::remove_file(&partial).ok();
                return Err(e);
            }
        }
    }
    if let Err(e) = std::fs::rename(&partial, &path) {
        std::fs::remove_file(&partial).ok();
        return Err(e).context("name the backup");
    }
    trim_backups(&dir, reason)?;
    tracing::info!(path = %path.display(), "backup written");
    Ok(path)
}

/// Opens a SQLite file outside the data folder and runs the migrations, for
/// backups of a Postgres database and for `db migrate`.
pub async fn open_sqlite_file(path: &Path) -> anyhow::Result<Pool> {
    let sqlite = SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options(path))
        .await
        .with_context(|| format!("open database {}", path.display()))?;
    MIGRATOR.run(&sqlite).await.context("run migrations")?;
    Ok(Pool::Sqlite(sqlite))
}

/// Copies a whole database into a new SQLite file at `path`. A Postgres
/// backup is such a file, so it also restores into local SQLite.
pub async fn export_to_sqlite(pool: &Pool, path: &Path) -> anyhow::Result<()> {
    let file = open_sqlite_file(path).await?;
    let copied = async {
        let mut from = pool.begin().await?;
        let mut to = file.begin_write().await?;
        crate::store::transfer::copy_workspace(&mut from, &mut to, None)
            .await
            .map_err(|e| anyhow::anyhow!("copy the database: {}", e.message))?;
        to.commit().await?;
        from.rollback().await?;
        anyhow::Ok(())
    }
    .await;
    file.close().await;
    copied
}

/// Copies a whole workspace from `from` to `to` for `llmtodo db migrate`
/// (LLM-462), then checks that every table holds as many rows as the copy
/// wrote. No server may write to either database meanwhile.
///
/// `to` must hold no issues unless `replace` is set; its rows go first
/// either way. `files_dir` is the source's `files/` folder.
pub async fn copy_database(
    from: &Pool,
    to: &Pool,
    files_dir: Option<&Path>,
    replace: bool,
) -> anyhow::Result<crate::store::transfer::Report> {
    use crate::store::transfer;
    let mut tx = to.begin_write().await?;
    let existing = transfer::issue_count(&mut tx)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    if existing > 0 && !replace {
        anyhow::bail!(
            "the target database already holds {existing} issues; pass --replace to write over them"
        );
    }
    if existing > 0 {
        // copy_workspace refuses a target with issues, so they go first.
        crate::store::backend::query("DELETE FROM issues")
            .execute(&mut tx)
            .await?;
    }
    let mut src = from.begin().await?;
    let report = transfer::copy_workspace(&mut src, &mut tx, files_dir)
        .await
        .map_err(|e| anyhow::anyhow!("copy the database: {}", e.message))?;
    src.rollback().await?;
    let counts = transfer::counts(&mut tx)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    for (table, n) in &counts {
        let copied = report.rows(table);
        if *n != copied {
            anyhow::bail!(
                "{table}: copied {copied} rows but the target holds {n}; nothing was changed"
            );
        }
    }
    tx.commit().await?;
    Ok(report)
}

pub async fn vacuum_into(pool: &Pool, path: &Path) -> anyhow::Result<()> {
    let pool = pool
        .sqlite()
        .context("VACUUM INTO needs a SQLite database")?;
    let mut conn = pool.acquire().await?;
    vacuum_into_conn(&mut conn, path).await
}

pub async fn vacuum_into_conn(conn: &mut SqliteConnection, path: &Path) -> anyhow::Result<()> {
    let target = path.to_str().context("backup path is not UTF-8")?;
    let done = sqlx::query("VACUUM INTO ?")
        .bind(target)
        .execute(&mut *conn)
        .await;
    if let Err(e) = done {
        // A failed copy, such as on a full disk, can leave part of a file.
        std::fs::remove_file(path).ok();
        let mut journal = path.as_os_str().to_owned();
        journal.push("-journal");
        std::fs::remove_file(journal).ok();
        return Err(e).context("VACUUM INTO");
    }
    Ok(())
}

fn trim_backups(dir: &Path, reason: &str) -> anyhow::Result<()> {
    let prefix = format!("llmtodo-{reason}-");
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".db"))
        })
        .collect();
    files.sort();
    while files.len() > KEEP_BACKUPS {
        let old = files.remove(0);
        std::fs::remove_file(&old).ok();
    }
    Ok(())
}

/// The newest daily backup's age, or None when there is none.
pub fn newest_daily_backup(data_dir: &Path) -> Option<std::time::SystemTime> {
    let dir = data_dir.join("backups");
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("llmtodo-daily-")
                && e.file_name().to_string_lossy().ends_with(".db")
        })
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
}

pub async fn get_setting(pool: &Pool, key: &str) -> anyhow::Result<Option<String>> {
    Ok(
        backend::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn set_setting(
    conn: &mut crate::store::backend::Conn,
    key: &str,
    value: &str,
) -> anyhow::Result<()> {
    backend::query("INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value")
        .bind(key)
        .bind(value)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Listens for writes by other servers on the shared Postgres database and
/// sends their events to this server's clients (LLM-459). It holds one
/// connection of its own, which must not go through a transaction pooler
/// such as PgBouncer. A lost connection reconnects after a pause, and the
/// first notice after that catches up from the last seq sent.
pub async fn listen_loop(state: crate::state::AppState) {
    use sqlx::postgres::PgListener;
    // Enabled now, so a stop that comes while this loop sends still ends it.
    let shutdown = state.shutdown.notified();
    tokio::pin!(shutdown);
    shutdown.as_mut().enable();
    let mut pause = Duration::from_millis(500);
    loop {
        let Some(pg) = state.pool().postgres().cloned() else {
            return;
        };
        if pg.is_closed() {
            return;
        }
        let listener = async {
            let mut l = PgListener::connect_with(&pg).await?;
            l.listen(crate::store::activity::CHANNEL).await?;
            Ok::<_, sqlx::Error>(l)
        }
        .await;
        match listener {
            Ok(mut l) => {
                pause = Duration::from_millis(500);
                // Writes made while no connection listened.
                crate::state::send_remote(&state).await;
                // The loop also looks at the pool, which the stop closes: an
                // open listener would keep that close waiting.
                let mut tick = tokio::time::interval(Duration::from_millis(250));
                loop {
                    tokio::select! {
                        n = l.recv() => match n {
                            Ok(_) => crate::state::send_remote(&state).await,
                            Err(e) => {
                                tracing::warn!(error = %pg_error(&e), "the Postgres listen connection dropped");
                                break;
                            }
                        },
                        _ = tick.tick() => {
                            if pg.is_closed() {
                                return;
                            }
                        }
                        _ = &mut shutdown => return,
                    }
                }
            }
            Err(e) => tracing::warn!(error = %pg_error(&e), "could not listen on Postgres"),
        }
        tokio::select! {
            _ = tokio::time::sleep(pause) => {}
            _ = &mut shutdown => return,
        }
        pause = (pause * 2).min(Duration::from_secs(30));
    }
}
