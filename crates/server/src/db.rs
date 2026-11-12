//! Opening the database, migrations, backups, IDs and timestamps.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::Context;
use jiff::Timestamp;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{SqlitePool, sqlite::SqliteConnection};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

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
}

/// Opens the pool and runs pending migrations. A backup is written before any
/// migration of an existing database.
pub async fn open(data_dir: &Path) -> anyhow::Result<SqlitePool> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("create data folder {}", data_dir.display()))?;
    let path = db_path(data_dir);
    let existed = path.exists();
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options(&path))
        .await
        .with_context(|| format!("open database {}", path.display()))?;

    if existed {
        let applied: i64 =
            sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success = 1")
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
            sqlx::query_scalar("SELECT coalesce(max(version), 0) FROM _sqlx_migrations")
                .fetch_one(&pool)
                .await
                .unwrap_or(0);
        if newest_applied > newest_known {
            anyhow::bail!(
                "database schema {newest_applied} is newer than this build supports ({newest_known}); upgrade llmtodo"
            );
        }
    }
    MIGRATOR.run(&pool).await.context("run migrations")?;
    crate::ops::questions::backfill(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("fill the questions table: {}", e.message))?;
    Ok(pool)
}

pub async fn schema_version(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT coalesce(max(version), 0) FROM _sqlx_migrations WHERE success = 1")
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

/// Writes a consistent copy with `VACUUM INTO` and trims old copies of the
/// same kind.
pub async fn backup(pool: &SqlitePool, data_dir: &Path, reason: &str) -> anyhow::Result<PathBuf> {
    let dir = data_dir.join("backups");
    std::fs::create_dir_all(&dir)?;
    let stamp = Timestamp::now().strftime("%Y%m%dT%H%M%S%.3fZ");
    let path = dir.join(format!("llmtodo-{reason}-{stamp}.db"));
    // The copy gets its real name only once it is whole. A copy cut short by
    // a crash keeps the `.partial` name, so it never counts as a backup.
    let partial = path.with_extension("db.partial");
    vacuum_into(pool, &partial).await?;
    if let Err(e) = std::fs::rename(&partial, &path) {
        std::fs::remove_file(&partial).ok();
        return Err(e).context("name the backup");
    }
    trim_backups(&dir, reason)?;
    tracing::info!(path = %path.display(), "backup written");
    Ok(path)
}

pub async fn vacuum_into(pool: &SqlitePool, path: &Path) -> anyhow::Result<()> {
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

pub async fn get_setting(pool: &SqlitePool, key: &str) -> anyhow::Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn set_setting(
    conn: &mut SqliteConnection,
    key: &str,
    value: &str,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value")
        .bind(key)
        .bind(value)
        .execute(&mut *conn)
        .await?;
    Ok(())
}
