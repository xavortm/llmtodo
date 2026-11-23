//! Offline mode on a shared database (LLM-471).
//!
//! A server on Postgres keeps `mirror.db` in its data folder: a SQLite file
//! with the same schema, filled from the shared database. When the shared
//! database is out of reach, the server swaps its pool to the copy, so
//! reads go on. Each write runs on the copy and is recorded in the outbox
//! table there. When Postgres answers again, the outbox replays in order
//! through the router, on a second state over Postgres (see
//! [`AppState::replay_twin`]), with each change's original actor, session
//! and time. The rules for changes that clash live in [`prepare_update`].
//!
//! The copy refreshes after this server's own writes and after notices from
//! other servers, from the activity that names each changed record. Nothing
//! refreshes on a timer, so a host such as Neon can still suspend an idle
//! database (decision 51).
//!
//! This module sits at the `ops` level, beside `github.rs`: it calls `ops`
//! and `store`, and reaches the routes only through `Inner::router`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use llmtodo_api_types::routes::{self as route_table, Endpoint, Method, ep};
use llmtodo_api_types::{
    ActivityField, CONTEXT_HEADER, ClientContext, EntityType, ErrorBody, ErrorCode, ErrorDetail,
    Outbox, OutboxEntry, SyncState, WsMessage,
};
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::db;
use crate::error::{ApiError, ApiResult, LostDatabase};
use crate::state::{AppState, Ctx, Write};
use crate::store;
use crate::store::backend::Pool;

/// The copy's file name in the data folder.
pub const MIRROR_FILE: &str = "mirror.db";

/// The biggest request body offline mode buffers.
const BODY_LIMIT: usize = 64 * 1024 * 1024;

/// More changed events than this since the last refresh, and the copy is
/// written again whole instead.
const REFRESH_MAX_EVENTS: i64 = 2000;

/// The wait between a change and the refresh it causes, so a burst of
/// writes costs one refresh.
const REFRESH_AFTER: Duration = Duration::from_millis(500);

/// The first and the longest wait between tries to reach Postgres again.
const RETRY_FIRST: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(15);

/// How long one try to reach Postgres again may take.
const RETRY_CONNECT_WAIT: Duration = Duration::from_secs(10);

pub fn mirror_path(data_dir: &Path) -> PathBuf {
    data_dir.join(MIRROR_FILE)
}

// ---------------------------------------------------------------- replay

/// A change that replays: the time it was made, and for a create the ID
/// and provisional key the issue keeps.
#[derive(Debug, Clone, Default)]
pub struct Replay {
    pub at: String,
    pub issue_id: Option<String>,
    pub alias: Option<String>,
}

tokio::task_local! {
    static REPLAY: Replay;
}

/// The original time of the change that replays now, if one does.
pub fn replay_time() -> Option<String> {
    REPLAY.try_with(|r| r.at.clone()).ok()
}

/// True while an offline change replays.
pub fn replaying() -> bool {
    REPLAY.try_with(|_| ()).is_ok()
}

/// The ID and the provisional key of an issue created offline, while its
/// create replays.
pub fn replay_issue() -> Option<(String, Option<String>)> {
    REPLAY
        .try_with(|r| r.issue_id.clone().map(|id| (id, r.alias.clone())))
        .ok()
        .flatten()
}

// ---------------------------------------------------------------- state

/// What a server on a shared database keeps for offline mode.
pub struct Shared {
    url: String,
    /// The Postgres pool. `None` while a server that started offline has
    /// not reached Postgres yet.
    pg: std::sync::RwLock<Option<Pool>>,
    /// The copy, once it exists.
    mirror: std::sync::RwLock<Option<Pool>>,
    offline: AtomicBool,
    syncing: AtomicBool,
    waiting: AtomicU64,
    failed: AtomicU64,
    /// Held while a write runs offline and records its outbox row, and
    /// while the copy is refreshed or swapped. So no outbox row is lost
    /// and no write lands between a check and a swap.
    gate: tokio::sync::Mutex<()>,
    /// Held while the copy is written again whole, so two writes never
    /// share the partial file.
    rebuilding: tokio::sync::Mutex<()>,
    refresh: Notify,
    reconnecting: AtomicBool,
}

impl Shared {
    pub fn new(url: String, pg: Option<Pool>, mirror: Option<Pool>, offline: bool) -> Self {
        Shared {
            url,
            pg: std::sync::RwLock::new(pg),
            mirror: std::sync::RwLock::new(mirror),
            offline: AtomicBool::new(offline),
            syncing: AtomicBool::new(false),
            waiting: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            gate: tokio::sync::Mutex::new(()),
            rebuilding: tokio::sync::Mutex::new(()),
            refresh: Notify::new(),
            reconnecting: AtomicBool::new(false),
        }
    }

    pub fn is_offline(&self) -> bool {
        self.offline.load(Ordering::SeqCst)
    }

    pub fn pg(&self) -> Option<Pool> {
        self.pg.read().expect("pg lock").clone()
    }

    pub fn mirror(&self) -> Option<Pool> {
        self.mirror.read().expect("mirror lock").clone()
    }

    pub fn state(&self) -> SyncState {
        SyncState {
            offline: self.is_offline(),
            syncing: self.syncing.load(Ordering::SeqCst),
            waiting: self.waiting.load(Ordering::SeqCst),
            failed: self.failed.load(Ordering::SeqCst),
        }
    }

    /// The shared database changed: refresh the copy soon.
    pub fn changed(&self) {
        self.refresh.notify_one();
    }
}

/// Reads the outbox counts into `shared`.
async fn count(shared: &Shared) {
    let Some(m) = shared.mirror() else { return };
    let Ok(mut c) = m.acquire().await else { return };
    if let Ok((w, f)) = store::outbox::counts(&mut c).await {
        shared.waiting.store(w, Ordering::SeqCst);
        shared.failed.store(f, Ordering::SeqCst);
    }
}

/// Tells live clients where the server stands.
fn announce(state: &AppState) {
    if let Some(s) = &state.shared {
        let _ = state.events.send(WsMessage::Sync { state: s.state() });
    }
}

// ---------------------------------------------------------------- start

/// How a server on a shared database starts.
pub struct Start {
    /// The pool to serve: Postgres, or the copy when offline.
    pub pool: Pool,
    pub shared: Shared,
}

/// Opens the shared database, or the copy when Postgres is out of reach.
/// A copy with changes still waiting starts offline too, and replays them
/// at once. With no copy, an unreachable database fails the start, as
/// before (LLM-470).
pub async fn start(url: &str, data_dir: &Path) -> anyhow::Result<Start> {
    let path = mirror_path(data_dir);
    let mirror = if path.exists() {
        match db::open_mirror_file(&path).await {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::warn!(error = %e, "the offline copy does not open; it will be written again");
                std::fs::remove_file(&path).ok();
                None
            }
        }
    } else {
        None
    };
    let pending = match &mirror {
        Some(m) => {
            let mut c = m.acquire().await?;
            store::outbox::counts(&mut c)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.message))?
                .0
        }
        None => 0,
    };
    match db::open_postgres(url, Some(data_dir)).await {
        Ok(pg) => {
            let offline = pending > 0;
            let pool = match (&mirror, offline) {
                (Some(m), true) => m.clone(),
                _ => pg.clone(),
            };
            Ok(Start {
                pool,
                shared: Shared::new(url.to_string(), Some(pg), mirror, offline),
            })
        }
        Err(e) if e.is::<db::Unreachable>() => match mirror {
            Some(m) => {
                tracing::warn!(error = %e, "starting offline on the local copy");
                Ok(Start {
                    pool: m.clone(),
                    shared: Shared::new(url.to_string(), None, Some(m), true),
                })
            }
            None => Err(e),
        },
        Err(e) => Err(e),
    }
}

/// The background work of offline mode: the first copy, the refresh loop,
/// and a replay when the server started offline.
pub fn spawn(state: &AppState) {
    let Some(shared) = state.shared.clone() else {
        return;
    };
    let st = state.clone();
    tokio::spawn(async move {
        count(&shared).await;
        if shared.is_offline() {
            announce(&st);
            reconnect(&st);
        } else if let Err(e) = rebuild(&st).await {
            tracing::warn!(error = %e, "could not write the offline copy");
        }
        refresh_loop(st).await;
    });
}

// ---------------------------------------------------------------- the copy

/// Writes the copy again, whole, from Postgres. The new file gets its name
/// only once it is complete, and takes the outbox rows of the old one.
pub async fn rebuild(state: &AppState) -> anyhow::Result<()> {
    let shared = state.shared.as_ref().expect("offline mode");
    let pg = shared
        .pg()
        .ok_or_else(|| anyhow::anyhow!("no Postgres pool"))?;
    let _rebuilding = shared.rebuilding.lock().await;
    let path = mirror_path(&state.data_dir);
    let partial = path.with_extension("db.partial");
    remove_sqlite_files(&partial);
    let file = db::open_mirror_file(&partial).await?;
    let copied = async {
        let mut from = pg.begin().await?;
        let mut to = file.begin_write().await?;
        store::transfer::copy_workspace(&mut from, &mut to, None)
            .await
            .map_err(|e| anyhow::anyhow!("copy the shared database: {}", e.message))?;
        to.commit().await?;
        from.rollback().await?;
        anyhow::Ok(())
    }
    .await;
    if let Err(e) = copied {
        file.close().await;
        remove_sqlite_files(&partial);
        return Err(e);
    }
    let _gate = shared.gate.lock().await;
    let old = shared.mirror();
    if let Some(old) = &old {
        let mut from = old.acquire().await?;
        let mut to = file.acquire().await?;
        store::outbox::carry(&mut from, &mut to)
            .await
            .map_err(|e| anyhow::anyhow!("carry the outbox: {}", e.message))?;
    }
    if shared.is_offline() {
        // The copy serves requests now, so it stays. This one is late.
        file.close().await;
        remove_sqlite_files(&partial);
        return Ok(());
    }
    file.close().await;
    if let Some(old) = old {
        old.close().await;
    }
    remove_sqlite_files(&path);
    std::fs::rename(&partial, &path)?;
    *shared.mirror.write().expect("mirror lock") = Some(db::open_mirror_file(&path).await?);
    drop(_gate);
    count(shared).await;
    tracing::info!(path = %path.display(), "offline copy written");
    Ok(())
}

/// Removes a SQLite file with its write-ahead log, so a new file under the
/// name never meets an old log.
fn remove_sqlite_files(path: &Path) {
    std::fs::remove_file(path).ok();
    for ext in ["-wal", "-shm", "-journal"] {
        let mut p = path.as_os_str().to_owned();
        p.push(ext);
        std::fs::remove_file(PathBuf::from(p)).ok();
    }
}

/// Copies the records that changed on Postgres since the copy's newest
/// activity seq: each named issue's rows, and the catalog when an event
/// names no issue.
async fn refresh(state: &AppState) -> anyhow::Result<()> {
    let shared = state.shared.as_ref().expect("offline mode");
    let (Some(pg), Some(mirror)) = (shared.pg(), shared.mirror()) else {
        // The first copy is still being written; it will be current.
        if shared.rebuilding.try_lock().is_err() {
            return Ok(());
        }
        return rebuild(state).await;
    };
    let gate = shared.gate.lock().await;
    if shared.is_offline() {
        return Ok(());
    }
    let mut m = mirror.acquire().await?;
    let seq = store::outbox::max_seq(&mut m)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    let mut p = pg.acquire().await?;
    let changed = store::outbox::changed_after(&mut p, seq, REFRESH_MAX_EVENTS + 1)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    if changed.is_empty() {
        return Ok(());
    }
    if changed.len() as i64 > REFRESH_MAX_EVENTS {
        drop((m, p, gate));
        return rebuild(state).await;
    }
    let upto = changed.last().map_or(seq, |(s, _)| *s);
    let mut issues: Vec<String> = changed.iter().filter_map(|(_, i)| i.clone()).collect();
    issues.sort();
    issues.dedup();
    let catalog = changed.iter().any(|(_, i)| i.is_none());
    // Rows arrive table by table, so a parent may come after its child.
    // With foreign keys off, a delete also never clears another issue's
    // parent.
    store::backend::query("PRAGMA foreign_keys = OFF")
        .execute(&mut m)
        .await?;
    store::backend::query("BEGIN IMMEDIATE")
        .execute(&mut m)
        .await?;
    let copied = async {
        store::transfer::refresh_issues(&mut p, &mut m, &issues).await?;
        if catalog {
            store::transfer::refresh_catalog(&mut p, &mut m).await?;
        }
        store::transfer::refresh_activity(&mut p, &mut m, seq, upto).await?;
        ApiResult::Ok(())
    }
    .await;
    let end = if copied.is_ok() { "COMMIT" } else { "ROLLBACK" };
    store::backend::query(end).execute(&mut m).await?;
    store::backend::query("PRAGMA foreign_keys = ON")
        .execute(&mut m)
        .await?;
    copied.map_err(|e| anyhow::anyhow!("refresh the offline copy: {}", e.message))?;
    Ok(())
}

async fn refresh_loop(state: AppState) {
    let Some(shared) = state.shared.clone() else {
        return;
    };
    let shutdown = state.shutdown.notified();
    tokio::pin!(shutdown);
    shutdown.as_mut().enable();
    loop {
        tokio::select! {
            _ = shared.refresh.notified() => {}
            _ = &mut shutdown => return,
        }
        tokio::select! {
            _ = tokio::time::sleep(REFRESH_AFTER) => {}
            _ = &mut shutdown => return,
        }
        if shared.is_offline() {
            continue;
        }
        if let Err(e) = refresh(&state).await {
            tracing::warn!(error = %e, "could not refresh the offline copy");
        }
    }
}

// ---------------------------------------------------------------- offline

/// Swaps to the copy. False when there is no copy, so the caller reports
/// the lost database as before.
pub async fn go_offline(state: &AppState, reason: &str) -> bool {
    let Some(shared) = state.shared.clone() else {
        return false;
    };
    if shared.is_offline() {
        return true;
    }
    let Some(mirror) = shared.mirror() else {
        return false;
    };
    {
        let _gate = shared.gate.lock().await;
        if shared.is_offline() {
            return true;
        }
        let _writes = state.writes.write().await;
        let seq = match mirror.acquire().await {
            Ok(mut c) => store::outbox::max_seq(&mut c).await.unwrap_or(0),
            Err(_) => return false,
        };
        state.set_pool(mirror);
        *state.sent_seq.lock().await = seq;
        shared.offline.store(true, Ordering::SeqCst);
    }
    tracing::warn!(
        reason,
        "the shared database is out of reach; working offline"
    );
    count(&shared).await;
    crate::state::send_reload(state);
    announce(state);
    reconnect(state);
    true
}

/// Starts trying Postgres again, unless a try already runs.
fn reconnect(state: &AppState) {
    let Some(shared) = state.shared.clone() else {
        return;
    };
    if shared.reconnecting.swap(true, Ordering::SeqCst) {
        return;
    }
    let st = state.clone();
    tokio::spawn(async move {
        let shutdown = st.shutdown.notified();
        tokio::pin!(shutdown);
        shutdown.as_mut().enable();
        let mut pause = RETRY_FIRST;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(pause) => {}
                _ = &mut shutdown => break,
            }
            pause = (pause * 2).min(RETRY_MAX);
            if !db::reachable(&shared.url, RETRY_CONNECT_WAIT).await {
                continue;
            }
            let pool = match db::open_postgres(&shared.url, Some(&st.data_dir)).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::info!(error = %e, "Postgres answered but did not open");
                    continue;
                }
            };
            match go_online(&st, pool).await {
                Ok(()) => break,
                Err(reason) => tracing::info!(reason, "replay stopped; still offline"),
            }
        }
        shared.reconnecting.store(false, Ordering::SeqCst);
    });
}

/// Replays the outbox on `pg`, then swaps back to it. An error is why the
/// replay stopped, with the server still offline.
async fn go_online(state: &AppState, pg: Pool) -> Result<(), String> {
    let shared = state.shared.clone().expect("offline mode");
    // The old pool's connections died with the network.
    let old = shared.pg.write().expect("pg lock").replace(pg.clone());
    if let Some(old) = old {
        tokio::spawn(async move { old.close().await });
    }
    shared.syncing.store(true, Ordering::SeqCst);
    announce(state);
    let result = replay_all(state, &shared, &pg).await;
    if let Err(reason) = result {
        shared.syncing.store(false, Ordering::SeqCst);
        announce(state);
        return Err(reason);
    }
    tracing::info!("the shared database is back; working online");
    crate::state::send_reload(state);
    if let Err(e) = rebuild(state).await {
        tracing::warn!(error = %e, "could not write the offline copy");
    }
    shared.syncing.store(false, Ordering::SeqCst);
    count(&shared).await;
    announce(state);
    Ok(())
}

/// Replays every pending outbox row in order, then, with the gate held and
/// none left, swaps the server back to Postgres.
async fn replay_all(state: &AppState, shared: &Arc<Shared>, pg: &Pool) -> Result<(), String> {
    let twin = state.replay_twin(pg.clone());
    let make = state
        .router
        .get()
        .copied()
        .ok_or_else(|| "no router to replay through".to_string())?;
    let router = make(twin.clone());
    let mut last = 0i64;
    loop {
        let row = {
            let _gate = shared.gate.lock().await;
            let mirror = shared.mirror().ok_or("no offline copy")?;
            let mut c = mirror.acquire().await.map_err(|e| e.to_string())?;
            let row = store::outbox::next_pending(&mut c, last)
                .await
                .map_err(|e| e.message)?;
            if row.is_none() {
                // Nothing waits: swap now, before another write comes.
                let _writes = state.writes.write().await;
                let seq = {
                    let mut c = pg.acquire().await.map_err(|e| e.to_string())?;
                    store::outbox::max_seq(&mut c)
                        .await
                        .map_err(|e| e.message)?
                };
                state.set_pool(pg.clone());
                *state.sent_seq.lock().await = seq;
                shared.offline.store(false, Ordering::SeqCst);
                return Ok(());
            }
            row
        };
        let row = row.expect("a row");
        last = row.id;
        let outcome = replay_one(&twin, router.clone(), &row).await;
        let _gate = shared.gate.lock().await;
        let mirror = shared.mirror().ok_or("no offline copy")?;
        let mut c = mirror.acquire().await.map_err(|e| e.to_string())?;
        match outcome {
            Outcome::Applied => {
                store::outbox::delete(&mut c, row.id)
                    .await
                    .map_err(|e| e.message)?;
            }
            Outcome::Refused(err) => {
                tracing::warn!(id = row.id, path = %row.path, error = %err.message, "the shared database refused an offline change");
                let text = serde_json::to_string(&err).unwrap_or_default();
                store::outbox::mark_failed(&mut c, row.id, &text)
                    .await
                    .map_err(|e| e.message)?;
            }
            Outcome::Lost(reason) => return Err(reason),
        }
        drop(c);
        drop(_gate);
        count(shared).await;
        announce(state);
    }
}

enum Outcome {
    Applied,
    Refused(ErrorDetail),
    Lost(String),
}

fn refused(e: ApiError) -> Outcome {
    if e.code == ErrorCode::DatabaseUnreachable {
        return Outcome::Lost(e.message);
    }
    Outcome::Refused(ErrorDetail {
        code: e.code,
        message: e.message,
        details: e.details,
    })
}

/// The route a stored row names.
fn route_of(method: &str, path: &str) -> Option<&'static route_table::Route> {
    let m = parse_method(method)?;
    route_table::find(m, path)
}

fn parse_method(m: &str) -> Option<Method> {
    Some(match m {
        "GET" => Method::Get,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "PATCH" => Method::Patch,
        "DELETE" => Method::Delete,
        _ => return None,
    })
}

fn is<E: Endpoint>(r: &route_table::Route) -> bool {
    r.method == E::METHOD && r.path == E::PATH
}

/// The issue a path names, such as `LLM-12` in `/v1/issues/LLM-12/claim`.
fn path_issue(path: &str) -> Option<&str> {
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    match segs.as_slice() {
        ["v1", "issues", r, ..] if !matches!(*r, "query" | "frontier") => Some(r),
        _ => None,
    }
}

/// The caller of a stored row, for the writes replay makes itself.
async fn ctx_of(twin: &AppState, context: Option<&str>) -> ApiResult<Ctx> {
    use base64::Engine;
    let client: ClientContext = match context {
        None => ClientContext::default(),
        Some(v) => {
            let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(v.as_bytes())
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            serde_json::from_slice(&raw)?
        }
    };
    let actor = store::actors::resolve_input(&twin.pool(), &client.actor).await?;
    Ok(Ctx { client, actor })
}

async fn replay_one(twin: &AppState, router: axum::Router, row: &store::outbox::Row) -> Outcome {
    use tower::ServiceExt as _;
    let Some(route) = route_of(&row.method, &row.path) else {
        return Outcome::Refused(ErrorDetail {
            code: ErrorCode::NotFound,
            message: format!("no route for {} {}", row.method, row.path),
            details: Value::Null,
        });
    };
    let mut body: Value = row
        .body
        .as_deref()
        .and_then(|b| serde_json::from_str(b).ok())
        .unwrap_or(Value::Null);
    let after: Value = row
        .after
        .as_deref()
        .and_then(|a| serde_json::from_str(a).ok())
        .unwrap_or(Value::Null);
    let mut replay = Replay {
        at: row.created_at.clone(),
        issue_id: None,
        alias: None,
    };
    if is::<ep::CreateIssue>(route) {
        replay.issue_id = after["created"]["id"].as_str().map(str::to_string);
        replay.alias = after["created"]["key"].as_str().map(str::to_string);
    }
    let mut lost = None;
    if is::<ep::UpdateIssue>(route) {
        match prepare_update(twin, row, &mut body, &after).await {
            Ok(Prepared::Send(l)) => lost = l,
            Ok(Prepared::Skip(l)) => {
                if let Some(l) = l
                    && let Err(e) = record_lost(twin, row, l).await
                {
                    return refused(e);
                }
                return Outcome::Applied;
            }
            Err(e) => return refused(e),
        }
    }
    let mut req = Request::builder()
        .method(row.method.as_str())
        .uri(row.path.as_str())
        .header(header::HOST, "127.0.0.1")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(c) = &row.context
        && let Ok(v) = HeaderValue::from_str(c)
    {
        req = req.header(CONTEXT_HEADER, v);
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    let Ok(req) = req.body(body) else {
        return Outcome::Refused(ErrorDetail {
            code: ErrorCode::BadRequest,
            message: "the stored request does not form a request".into(),
            details: Value::Null,
        });
    };
    let res = match REPLAY.scope(replay, router.oneshot(req)).await {
        Ok(r) => r,
        Err(never) => match never {},
    };
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), BODY_LIMIT)
        .await
        .unwrap_or_default();
    if status.is_success() {
        if let Some(l) = lost
            && let Err(e) = record_lost(twin, row, l).await
        {
            tracing::warn!(error = %e.message, "could not record a lost offline change");
        }
        return Outcome::Applied;
    }
    let err = serde_json::from_slice::<ErrorBody>(&bytes)
        .map(|b| b.error)
        .unwrap_or_else(|_| ErrorDetail {
            code: ErrorCode::Internal,
            message: format!("replay answered {status}"),
            details: Value::Null,
        });
    if err.code == ErrorCode::DatabaseUnreachable {
        return Outcome::Lost(err.message);
    }
    Outcome::Refused(err)
}

/// Each group of fields replay compares: its key in
/// `store::outbox::snapshot`, the `UpdateIssue` fields that set it, and the
/// activity fields that record a change to it.
const FIELDS: &[(&str, &[&str], &[&str])] = &[
    ("title", &["title"], &["title"]),
    ("status", &["status"], &["status"]),
    ("priority", &["priority"], &["priority"]),
    ("assignee", &["assignee"], &["assignee"]),
    (
        "labels",
        &["labels", "add_labels", "remove_labels"],
        &["labels"],
    ),
    ("project", &["project"], &["project"]),
    ("milestone", &["milestone"], &["milestone"]),
    ("parent", &["parent"], &["parent"]),
    ("order_step", &["order_step"], &["order"]),
    ("team", &["team"], &["key"]),
    ("waiting_on", &["waiting_on"], &["waiting_on"]),
    (
        "wanted_executor",
        &["wanted_executor"],
        &["wanted_executor"],
    ),
    (
        "estimates",
        &["estimate_minutes", "estimate_cost_usd", "estimate_reason"],
        &["estimate_minutes", "estimate_cost_usd"],
    ),
    (
        "git",
        &["branch", "worktree", "pr"],
        &["git_branch", "git_worktree", "git_pr"],
    ),
];

/// The description fields of `UpdateIssue`.
const BODY_FIELDS: &[&str] = &["body", "base_version", "force", "patches"];

/// Field groups the shared database kept over an offline change: what the
/// change asked for, and what stayed.
struct Lost {
    issue_id: String,
    offline: serde_json::Map<String, Value>,
    kept: serde_json::Map<String, Value>,
}

enum Prepared {
    Send(Option<Lost>),
    /// Nothing is left to send.
    Skip(Option<Lost>),
}

/// Applies the conflict rules to an issue update before it replays:
///
/// - **A field** applies when Postgres still holds the value the change
///   saw. When the other side changed it meanwhile, the later change by
///   time wins. A lost change is recorded as a `sync_conflict` event.
/// - **The description** applies as written when nobody else changed it.
///   Otherwise both edits merge three ways from the text the change saw.
///   When both changed the same lines, the shared text stays, the offline
///   text goes into a `+++ Sync conflict` section, and the issue waits on
///   the owner.
/// - **A trashed issue** takes the edits anyway, so the trash keeps them.
async fn prepare_update(
    twin: &AppState,
    row: &store::outbox::Row,
    body: &mut Value,
    after: &Value,
) -> ApiResult<Prepared> {
    let Some(r) = path_issue(&row.path) else {
        return Ok(Prepared::Send(None));
    };
    let Some(obj) = body.as_object_mut() else {
        return Ok(Prepared::Send(None));
    };
    let mut c = twin.pool().acquire().await?;
    let id = store::issues::resolve_id(&mut c, r).await?;
    let now = store::outbox::snapshot(&mut c, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("issue", r))?;
    let seen: Value = row
        .seen
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let mut lost = Lost {
        issue_id: id.clone(),
        offline: Default::default(),
        kept: Default::default(),
    };
    for (group, keys, fields) in FIELDS {
        if !keys.iter().any(|k| obj.contains_key(*k)) || seen.is_null() {
            continue;
        }
        if seen[*group] == now[*group] {
            continue;
        }
        let theirs = store::outbox::last_change(&mut c, &id, fields).await?;
        if theirs.is_some_and(|t| t > row.created_at) {
            let mut asked = serde_json::Map::new();
            for k in *keys {
                if let Some(v) = obj.remove(*k) {
                    asked.insert(k.to_string(), v);
                }
            }
            lost.offline.insert(group.to_string(), Value::Object(asked));
            lost.kept.insert(group.to_string(), now[*group].clone());
        }
    }
    if BODY_FIELDS.iter().any(|k| obj.contains_key(*k)) && !seen.is_null() {
        for k in BODY_FIELDS {
            obj.remove(*k);
        }
        let base = seen["description"].as_str().unwrap_or_default();
        let ours = after["description"].as_str().unwrap_or(base);
        let theirs = now["description"].as_str().unwrap_or_default();
        let version = now["description_version"].as_i64().unwrap_or(1);
        if ours != theirs {
            let text = if theirs == base {
                Some(ours.to_string())
            } else {
                crate::merge::merge3(base, ours, theirs)
            };
            let text = match text {
                Some(t) => t,
                None => {
                    let who = ctx_of(twin, row.context.as_deref())
                        .await
                        .map(|c| c.actor.name)
                        .unwrap_or_else(|_| "Someone".into());
                    obj.insert("waiting_on".into(), json!("owner"));
                    crate::merge::conflict_section(theirs, ours, &row.created_at, &who)
                }
            };
            obj.insert("body".into(), json!(text));
            obj.insert("base_version".into(), json!(version));
        }
    }
    let lost = (!lost.offline.is_empty()).then_some(lost);
    if obj.is_empty() {
        return Ok(Prepared::Skip(lost));
    }
    Ok(Prepared::Send(lost))
}

/// Records the field changes the shared database kept over, so the
/// activity log shows both.
async fn record_lost(twin: &AppState, row: &store::outbox::Row, lost: Lost) -> ApiResult<()> {
    let ctx = ctx_of(twin, row.context.as_deref()).await?;
    let replay = Replay {
        at: row.created_at.clone(),
        ..Default::default()
    };
    REPLAY
        .scope(replay, async {
            let mut w = Write::begin(twin, &ctx).await?;
            w.event(
                EntityType::Issue,
                &lost.issue_id,
                Some(&lost.issue_id),
                ActivityField::SyncConflict,
                Some(Value::Object(lost.kept)),
                Some(Value::Object(lost.offline)),
            )
            .await?;
            w.commit().await
        })
        .await
}

// ---------------------------------------------------------------- requests

/// Routes that need the shared database itself. Two laptops could both win
/// a claim made offline, and the rest act on the shared database or on
/// outside services.
fn online_only(r: &route_table::Route) -> bool {
    is::<ep::Claim>(r)
        || is::<ep::Release>(r)
        || is::<ep::SyncPush>(r)
        || is::<ep::SyncPull>(r)
        || is::<ep::SyncStatus>(r)
        || is::<ep::Prune>(r)
        || is::<ep::ImportLinear>(r)
        || is::<ep::GithubSync>(r)
}

/// Writes that act on this server only, so the outbox never holds them.
fn local_only(r: &route_table::Route) -> bool {
    is::<ep::StopServer>(r) || is::<ep::Backup>(r) || is::<ep::DropOutbox>(r)
}

fn needs_network(r: &route_table::Route) -> Response {
    let message = if is::<ep::Claim>(r) || is::<ep::Release>(r) {
        "Claims need the shared database. Try again when you are online.".to_string()
    } else {
        "This command needs the shared database. Try again when you are online.".to_string()
    };
    ApiError::new(ErrorCode::NeedsNetwork, message)
        .with_details(json!({ "offline": true }))
        .into_response()
}

/// The offline layer of the router. Online, it runs the request, and when
/// the request lost the shared database and a copy exists, it goes offline
/// and runs the request again on the copy. Offline, it refuses the online
/// only routes and records each write in the outbox.
pub async fn layer(state: AppState, req: Request, next: axum::middleware::Next) -> Response {
    let Some(shared) = state.shared.clone() else {
        return next.run(req).await;
    };
    let path = req.uri().path().to_string();
    let route = parse_method(req.method().as_str()).and_then(|m| route_table::find(m, &path));
    // The socket and the health check take care of themselves.
    if route.is_none_or(|r| is::<ep::Ws>(r) || is::<ep::Health>(r)) {
        return next.run(req).await;
    }
    let route = route.expect("a route");
    if shared.is_offline() {
        return offline_request(&state, &shared, route, req, next).await;
    }
    if shared.mirror().is_none() {
        return next.run(req).await;
    }
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, BODY_LIMIT).await else {
        return ApiError::bad_request("the request body is too large").into_response();
    };
    let again = Request::builder()
        .method(parts.method.clone())
        .uri(parts.uri.clone())
        .version(parts.version);
    let headers = parts.headers.clone();
    let res = next
        .clone()
        .run(Request::from_parts(parts, Body::from(bytes.clone())))
        .await;
    let Some(lost) = res.extensions().get::<LostDatabase>().cloned() else {
        return res;
    };
    if lost.commit_unknown || !go_offline(&state, &lost.reason).await {
        return res;
    }
    let mut again = match again.body(Body::from(bytes)) {
        Ok(r) => r,
        Err(_) => return res,
    };
    *again.headers_mut() = headers;
    offline_request(&state, &shared, route, again, next).await
}

async fn offline_request(
    state: &AppState,
    shared: &Arc<Shared>,
    route: &'static route_table::Route,
    req: Request,
    next: axum::middleware::Next,
) -> Response {
    if online_only(route) {
        return needs_network(route);
    }
    if !route.writes || local_only(route) || is::<ep::Outbox>(route) {
        return next.run(req).await;
    }
    let (mut parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, BODY_LIMIT).await else {
        return ApiError::bad_request("the request body is too large").into_response();
    };
    let _gate = shared.gate.lock().await;
    let at = db::now();
    let Some(mirror) = shared.mirror() else {
        return ApiError::internal("offline with no copy").into_response();
    };
    let mut c = match mirror.acquire().await {
        Ok(c) => c,
        Err(e) => return ApiError::from(e).into_response(),
    };
    // Provisional keys become IDs, which stay the same after replay. The
    // order of an epic names keys only, so it keeps them: their aliases
    // resolve after replay.
    let mut path = parts.uri.path().to_string();
    let mut body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    if !is::<ep::SetOrder>(route) {
        path = rewrite_path(&mut c, &path).await;
        rewrite_keys(&mut c, &mut body).await;
    }
    let issue_id = match path_issue(&path) {
        Some(r) => store::issues::resolve_id(&mut c, r).await.ok(),
        None => None,
    };
    let seen = match &issue_id {
        Some(id) if is::<ep::UpdateIssue>(route) => {
            store::outbox::snapshot(&mut c, id).await.ok().flatten()
        }
        _ => None,
    };
    drop(c);
    let query = parts
        .uri
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let full = format!("{path}{query}");
    if let Ok(uri) = full.parse() {
        parts.uri = uri;
    }
    let sent = if body.is_null() {
        bytes.to_vec()
    } else {
        body.to_string().into_bytes()
    };
    let context = parts
        .headers
        .get(CONTEXT_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let method = parts.method.as_str().to_string();
    let res = next
        .run(Request::from_parts(parts, Body::from(sent.clone())))
        .await;
    if !res.status().is_success() {
        return res;
    }
    let (res_parts, res_body) = res.into_parts();
    let Ok(res_bytes) = axum::body::to_bytes(res_body, BODY_LIMIT).await else {
        return ApiError::internal("the response body is too large").into_response();
    };
    let mut after = json!({});
    if is::<ep::CreateIssue>(route)
        && let Ok(v) = serde_json::from_slice::<Value>(&res_bytes)
    {
        after["created"] = json!({ "id": v["id"], "key": v["key"] });
    }
    let mut c = match mirror.acquire().await {
        Ok(c) => c,
        Err(e) => return ApiError::from(e).into_response(),
    };
    if seen.is_some()
        && let Some(id) = &issue_id
        && let Ok((text, _)) = store::issues::description_and_version(&mut c, id).await
    {
        after["description"] = json!(text);
    }
    let body_text = (!sent.is_empty()).then(|| String::from_utf8_lossy(&sent).to_string());
    let recorded = store::outbox::insert(
        &mut c,
        store::outbox::NewRow {
            method: &method,
            path: &full,
            body: body_text.as_deref(),
            context: context.as_deref(),
            issue_id: issue_id.as_deref(),
            seen: seen.map(|s| s.to_string()),
            after: Some(after.to_string()),
            created_at: &at,
        },
    )
    .await;
    drop(c);
    drop(_gate);
    if let Err(e) = recorded {
        tracing::error!(error = %e.message, "could not record an offline change");
        return e.into_response();
    }
    count(shared).await;
    announce(state);
    Response::from_parts(res_parts, Body::from(res_bytes))
}

/// The ID of a provisional key, from the copy.
async fn provisional_id(c: &mut store::backend::Conn, s: &str) -> Option<String> {
    let key = s.parse::<llmtodo_core::IssueKey>().ok()?;
    if !key.provisional {
        return None;
    }
    store::issues::resolve_id(c, s).await.ok()
}

async fn rewrite_path(c: &mut store::backend::Conn, path: &str) -> String {
    let mut out = Vec::new();
    for seg in path.split('/') {
        match provisional_id(c, seg).await {
            Some(id) => out.push(id),
            None => out.push(seg.to_string()),
        }
    }
    out.join("/")
}

/// Turns every string that is a provisional key into the issue's ID.
async fn rewrite_keys(c: &mut store::backend::Conn, v: &mut Value) {
    let mut stack = vec![v];
    while let Some(v) = stack.pop() {
        match v {
            Value::String(s) if s.contains('~') => {
                if let Some(id) = provisional_id(c, s).await {
                    *s = id;
                }
            }
            Value::Array(a) => stack.extend(a.iter_mut()),
            Value::Object(o) => stack.extend(o.values_mut()),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- outbox

/// The outbox, for `llmtodo db outbox`.
pub async fn outbox(state: &AppState) -> ApiResult<Outbox> {
    let Some(shared) = &state.shared else {
        return Ok(Outbox {
            state: SyncState::default(),
            entries: Vec::new(),
        });
    };
    let rows = match shared.mirror() {
        Some(m) => {
            let mut c = m.acquire().await?;
            store::outbox::all(&mut c).await?
        }
        None => Vec::new(),
    };
    let entries = rows
        .into_iter()
        .map(|r| OutboxEntry {
            id: r.id,
            method: r.method,
            issue: r
                .path
                .split('?')
                .next()
                .and_then(path_issue)
                .map(str::to_string),
            path: r.path,
            created_at: db::parse_ts(&r.created_at),
            state: r.state,
            error: r.error.and_then(|e| serde_json::from_str(&e).ok()),
            body: r
                .body
                .and_then(|b| serde_json::from_str(&b).ok())
                .unwrap_or(Value::Null),
        })
        .collect();
    Ok(Outbox {
        state: shared.state(),
        entries,
    })
}

/// Discards one outbox row, for `llmtodo db outbox drop`.
pub async fn drop_row(state: &AppState, id: i64) -> ApiResult<Outbox> {
    let Some(shared) = state.shared.clone() else {
        return Err(ApiError::not_found("outbox row", &id.to_string()));
    };
    {
        let _gate = shared.gate.lock().await;
        let mirror = shared
            .mirror()
            .ok_or_else(|| ApiError::not_found("outbox row", &id.to_string()))?;
        let mut c = mirror.acquire().await?;
        if !store::outbox::delete(&mut c, id).await? {
            return Err(ApiError::not_found("outbox row", &id.to_string()));
        }
    }
    count(&shared).await;
    announce(state);
    outbox(state).await
}

/// The health check's view, and a chance to go offline when its check
/// found the shared database away.
pub async fn health_state(state: &AppState, unreachable: Option<&str>) -> SyncState {
    let Some(shared) = &state.shared else {
        return SyncState::default();
    };
    if let Some(reason) = unreachable
        && !shared.is_offline()
    {
        go_offline(state, reason).await;
    }
    shared.state()
}
