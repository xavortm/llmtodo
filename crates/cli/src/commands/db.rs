//! Choose the database: the local SQLite file or a shared Postgres database
//! (LLM-461, LLM-462). See `llmtodo_client::database` for where the choice
//! and the connection string live.
//!
//! `migrate` and `use` stop the server, change the database it opens, and
//! start it again. `migrate` opens both databases in this process, through
//! the server library, because no server may write while it copies.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Subcommand, ValueEnum};
use llmtodo_api_types::{ErrorCode, ErrorDetail};
use llmtodo_client::database::{self as dbcfg, Backend};
use serde_json::json;

use crate::{App, CliError, CliResult};

#[derive(Clone, Copy, ValueEnum)]
pub enum Target {
    Sqlite,
    Postgres,
}

impl Target {
    fn backend(self) -> Backend {
        match self {
            Target::Sqlite => Backend::Sqlite,
            Target::Postgres => Backend::Postgres,
        }
    }
}

#[derive(Subcommand)]
pub enum DbCmd {
    /// Show the configured database and the one the server opened.
    Status,
    /// Store the shared database's connection string in the macOS Keychain.
    /// It reads the string from stdin, so it never shows in the shell
    /// history: `op read op://vault/item/url | llmtodo db set-url`.
    SetUrl,
    /// Remove the stored connection string.
    ClearUrl,
    /// Switch to a database without copying data. `postgres` joins a shared
    /// database that already holds the workspace: the local SQLite file
    /// moves to `backups/`, untouched.
    Use {
        #[arg(value_enum)]
        backend: Target,
    },
    /// Copy the whole workspace to the other database, then switch to it.
    /// A backup is written first. The source stays as it is, except that a
    /// local SQLite file moves to `backups/` after a move to Postgres.
    Migrate {
        /// The database to copy to.
        #[arg(long, value_enum)]
        to: Target,
        /// Write over a target that already holds issues.
        #[arg(long)]
        replace: bool,
    },
    /// List the changes made offline on a shared database: the ones that
    /// wait to replay, and the ones the shared database refused.
    Outbox {
        #[command(subcommand)]
        cmd: Option<OutboxCmd>,
    },
}

#[derive(Subcommand)]
pub enum OutboxCmd {
    /// Discard one change from the outbox, such as one the shared database
    /// refused. Only after the owner says so. It asks first; pass `-y`.
    Drop {
        /// The row's `id` from `llmtodo db outbox`.
        id: i64,
    },
}

pub async fn run(app: &App, cmd: DbCmd) -> CliResult {
    match cmd {
        DbCmd::Status => status(app).await,
        DbCmd::SetUrl => set_url(app),
        DbCmd::ClearUrl => {
            dbcfg::delete_keychain_url().map_err(CliError::Other)?;
            app.out.value(json!({ "stored": false }));
            Ok(())
        }
        DbCmd::Use { backend } => use_backend(app, backend.backend()).await,
        DbCmd::Migrate { to, replace } => migrate(app, to.backend(), replace).await,
        DbCmd::Outbox { cmd: None } => {
            let c = app.client().await?;
            app.out.ser(&c.outbox().await?);
            Ok(())
        }
        DbCmd::Outbox {
            cmd: Some(OutboxCmd::Drop { id }),
        } => {
            app.confirm(&format!(
                "Discard offline change {id}? It never reaches the shared database."
            ))?;
            let c = app.client().await?;
            app.out.ser(&c.drop_outbox(id).await?);
            Ok(())
        }
    }
}

/// `db use` and `db migrate` stop the server and open the databases
/// themselves. While the server works offline, or holds offline changes,
/// that would set the changes aside, so both refuse (LLM-471).
async fn check_online(app: &App) -> CliResult {
    let Some((_, health)) =
        llmtodo_client::find_server(&app.data_dir, &app.detected.identity.context).await
    else {
        return Ok(());
    };
    let sync = health.database.sync;
    if !sync.offline && sync.waiting == 0 && sync.failed == 0 {
        return Ok(());
    }
    let message = if sync.offline {
        "This command needs the shared database. Try again when you are online.".to_string()
    } else {
        format!(
            "{} offline changes wait in the outbox; let them replay, or drop them with `llmtodo db outbox drop`, first",
            sync.waiting + sync.failed
        )
    };
    Err(CliError::Client(llmtodo_client::ClientError::Api {
        status: ErrorCode::NeedsNetwork.http_status(),
        detail: ErrorDetail {
            code: ErrorCode::NeedsNetwork,
            message,
            details: json!({ "offline": sync.offline, "waiting": sync.waiting, "failed": sync.failed }),
        },
    }))
}

async fn status(app: &App) -> CliResult {
    let url = dbcfg::stored_url().ok().flatten();
    let running = llmtodo_client::find_server(&app.data_dir, &app.detected.identity.context)
        .await
        .map(|(_, h)| h.database);
    app.out.value(json!({
        "configured": dbcfg::configured_backend().as_str(),
        "applies_here": applies_here(app),
        "main_data_dir": dbcfg::main_data_dir(),
        "data_dir": app.data_dir,
        "url_stored": url.is_some(),
        "url_host": url.as_deref().map(llmtodo_server::db::url_host),
        "url_from": if std::env::var(dbcfg::DATABASE_URL_ENV).is_ok_and(|u| !u.is_empty()) {
            dbcfg::DATABASE_URL_ENV
        } else {
            "keychain"
        },
        "server": running,
    }));
    Ok(())
}

fn applies_here(app: &App) -> bool {
    dbcfg::applies_to(&app.data_dir)
}

fn set_url(app: &App) -> CliResult {
    if app.interactive() {
        eprintln!(
            "Paste the Postgres connection string and press Enter. It shows as you paste, and goes only to the Keychain:"
        );
    }
    let mut url = String::new();
    std::io::stdin().read_line(&mut url)?;
    let url = url.trim();
    dbcfg::check_url(url).map_err(CliError::Validation)?;
    dbcfg::store_keychain_url(url).map_err(CliError::Other)?;
    app.out.value(json!({
        "stored": true,
        "host": llmtodo_server::db::url_host(url),
        "next": "llmtodo db migrate --to postgres  (to move this laptop's data), or  llmtodo db use postgres  (to join a database that already holds it)",
    }));
    Ok(())
}

/// The commands change what the main data folder opens. A preview or test
/// folder never uses the shared database, so they refuse there.
fn check_main_folder(app: &App) -> CliResult {
    if applies_here(app) {
        return Ok(());
    }
    Err(CliError::Validation(format!(
        "the database choice applies only to the main data folder, {}; this command runs on {}",
        dbcfg::main_data_dir().display(),
        app.data_dir.display()
    )))
}

fn stored_url() -> CliResult<String> {
    dbcfg::stored_url()
        .map_err(CliError::Other)?
        .ok_or_else(|| {
            CliError::Validation(
                "no connection string is stored; run `llmtodo db set-url` first".into(),
            )
        })
}

/// Stops this folder's server, waits until it has let go of the database,
/// and returns its lock, still held (LLM-477).
///
/// Hold the lock until the config names the new database. The app starts a
/// server as soon as the old one stops. Without the lock held, that server
/// would read the old config and serve the old database during the copy.
/// While this command holds the lock, such a server exits at once, and the
/// app tries again later.
async fn stop_server(app: &App) -> CliResult<ServerLock> {
    let path = app.data_dir.join("server.lock");
    // Look often: once the old server lets go, the lock goes to whoever
    // tries first.
    for _ in 0..500 {
        // Ask again on each round: a server another client started after
        // the first stop holds the lock too.
        if let Some((c, _)) =
            llmtodo_client::find_server(&app.data_dir, &app.detected.identity.context).await
        {
            let _ = c.stop_server().await;
        }
        let lock = std::fs::File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        if lock.try_lock().is_ok() {
            return Ok(ServerLock(lock));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Err(CliError::Other(
        "the server did not stop within 10 seconds; run `llmtodo server stop` and try again".into(),
    ))
}

/// The data folder's `server.lock`, held by this command. Dropping it lets
/// a server start again.
struct ServerLock(#[allow(dead_code)] std::fs::File);

/// Lets go of `lock`, then starts the server on the new choice and returns
/// what it opened.
async fn start_server(app: &App, lock: ServerLock) -> CliResult<serde_json::Value> {
    drop(lock);
    let c = app.connect().await?;
    let h = c.health().await?;
    Ok(serde_json::to_value(h.database).unwrap_or_default())
}

fn stamp() -> String {
    jiff::Timestamp::now()
        .strftime("%Y%m%dT%H%M%S%.3fZ")
        .to_string()
}

/// Moves the local SQLite file, with its WAL files, into `backups/` under
/// `name`. Returns the new path, or `None` when there was no file.
fn set_aside(data_dir: &Path, name: &str) -> CliResult<Option<PathBuf>> {
    let db = llmtodo_server::db::db_path(data_dir);
    if !db.exists() {
        return Ok(None);
    }
    let dir = data_dir.join("backups");
    std::fs::create_dir_all(&dir)?;
    let to = dir.join(format!("llmtodo-{name}-{}.db", stamp()));
    std::fs::rename(&db, &to)?;
    for ext in ["-wal", "-shm"] {
        let mut from = db.as_os_str().to_owned();
        from.push(ext);
        let mut dest = to.as_os_str().to_owned();
        dest.push(ext);
        if Path::new(&from).exists() {
            std::fs::rename(&from, &dest)?;
        }
    }
    Ok(Some(to))
}

async fn local_issue_count(data_dir: &Path) -> CliResult<i64> {
    if !llmtodo_server::db::db_path(data_dir).exists() {
        return Ok(0);
    }
    let pool = llmtodo_server::db::open(data_dir)
        .await
        .map_err(|e| CliError::Other(format!("{e:#}")))?;
    let mut c = pool
        .acquire()
        .await
        .map_err(|e| CliError::Other(e.to_string()))?;
    let n = llmtodo_server::store::transfer::issue_count(&mut c)
        .await
        .map_err(|e| CliError::Other(e.message))?;
    drop(c);
    pool.close().await;
    Ok(n)
}

/// The database this folder uses now. `check_main_folder` ran first, so
/// the config's choice applies.
fn current(_app: &App) -> CliResult<Backend> {
    Ok(dbcfg::configured_backend())
}

async fn use_backend(app: &App, to: Backend) -> CliResult {
    check_main_folder(app)?;
    check_online(app).await?;
    let mut moved = None;
    let lock;
    if to == Backend::Postgres {
        let url = stored_url()?;
        // Fails early on a wrong URL, before anything changes.
        let pg = llmtodo_server::db::open_postgres(&url, Some(&app.data_dir))
            .await
            .map_err(|e| CliError::Other(format!("{e:#}")))?;
        pg.close().await;
        lock = stop_server(app).await?;
        let local = local_issue_count(&app.data_dir).await?;
        if local > 0 {
            app.confirm(&format!(
                "This laptop's SQLite file holds {local} issues. It moves to backups/ and the shared database takes over. Go on?"
            ))?;
        }
        moved = set_aside(&app.data_dir, "before-postgres")?;
    } else {
        lock = stop_server(app).await?;
    }
    let config = dbcfg::set_configured_backend(to).map_err(CliError::Other)?;
    let server = start_server(app, lock).await?;
    app.out.value(json!({
        "backend": to.as_str(),
        "config": config,
        "local_file_moved_to": moved,
        "server": server,
    }));
    Ok(())
}

async fn migrate(app: &App, to: Backend, replace: bool) -> CliResult {
    check_main_folder(app)?;
    check_online(app).await?;
    let from = current(app)?;
    if from == to {
        return Err(CliError::Validation(format!(
            "this laptop already uses {}; pick the other database",
            to.as_str()
        )));
    }
    let url = stored_url()?;
    app.confirm(&format!(
        "Copy the whole workspace from {} to {}, and switch to it? The server stops while it copies.",
        from.as_str(),
        to.as_str()
    ))?;
    let lock = stop_server(app).await?;
    let data_dir = &app.data_dir;
    let err = |e: anyhow::Error| CliError::Other(format!("{e:#}"));
    let report = match to {
        Backend::Postgres => {
            let src = llmtodo_server::db::open(data_dir).await.map_err(err)?;
            llmtodo_server::db::backup(&src, data_dir, "pre-db-migrate")
                .await
                .map_err(err)?;
            let pg = llmtodo_server::db::open_postgres(&url, Some(data_dir))
                .await
                .map_err(err)?;
            let report = llmtodo_server::db::copy_database(
                &src,
                &pg,
                Some(&data_dir.join("files")),
                replace,
            )
            .await;
            src.close().await;
            pg.close().await;
            report.map_err(err)?
        }
        Backend::Sqlite => {
            let pg = llmtodo_server::db::open_postgres(&url, Some(data_dir))
                .await
                .map_err(err)?;
            llmtodo_server::db::backup(&pg, data_dir, "pre-db-migrate")
                .await
                .map_err(err)?;
            let local = local_issue_count(data_dir).await?;
            if local > 0 && !replace {
                pg.close().await;
                return Err(CliError::Validation(format!(
                    "this laptop's SQLite file already holds {local} issues; pass --replace to move it to backups/ and copy over it"
                )));
            }
            let moved = set_aside(data_dir, "before-sqlite")?;
            let file = llmtodo_server::db::open(data_dir).await.map_err(err)?;
            let report = llmtodo_server::db::copy_database(&pg, &file, None, false).await;
            file.close().await;
            pg.close().await;
            if let Err(e) = report {
                // Put the old file back, so a failed copy changes nothing.
                if let Some(old) = moved {
                    let _ = set_aside(data_dir, "failed-copy");
                    std::fs::rename(old, llmtodo_server::db::db_path(data_dir))?;
                }
                return Err(err(e));
            }
            report.map_err(err)?
        }
    };
    let moved = if to == Backend::Postgres {
        set_aside(data_dir, "moved-to-postgres")?
    } else {
        None
    };
    let config = dbcfg::set_configured_backend(to).map_err(CliError::Other)?;
    let server = start_server(app, lock).await?;
    app.out.value(json!({
        "from": from.as_str(),
        "to": to.as_str(),
        "rows": report.tables.iter().map(|(t, n)| (t.clone(), *n)).collect::<std::collections::BTreeMap<_, _>>(),
        "config": config,
        "local_file_moved_to": moved,
        "server": server,
    }));
    Ok(())
}
