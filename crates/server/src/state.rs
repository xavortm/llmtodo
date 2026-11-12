//! Shared server state, request identity and the write transaction helper.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use base64::Engine;
use jiff::Timestamp;
use llmtodo_api_types::{
    ActivityField, Actor, ActorKind, CONTEXT_HEADER, ClientContext, EntityType, ErrorCode,
    WsMessage,
};
use sqlx::{Sqlite, SqlitePool, Transaction};
use tokio::sync::{Notify, OwnedMutexGuard, OwnedRwLockReadGuard, RwLock, broadcast};

use crate::error::{ApiError, ApiResult};
use crate::store;

#[derive(Clone)]
pub struct AppState(pub Arc<Inner>);

pub struct Inner {
    /// Replaced when `sync pull` swaps the database file.
    pub pool_slot: std::sync::RwLock<SqlitePool>,
    /// This machine's name, as the sync manifest records it.
    pub host: String,
    pub data_dir: PathBuf,
    pub events: broadcast::Sender<WsMessage>,
    /// The highest activity seq sent on `events`. A commit holds this lock
    /// while it sends, so events go out in seq order with no gap.
    pub sent_seq: tokio::sync::Mutex<i64>,
    pub started_at: Timestamp,
    pub shutdown: Notify,
    pub is_primary: AtomicBool,
    /// Each write holds a read guard for its whole transaction. Sync pull
    /// takes the write guard, so it waits for writes in flight to finish.
    pub writes: Arc<RwLock<()>>,
    /// Writers queue here before they take a pool connection. SQLite runs
    /// one write at a time, so a writer that waited on `BEGIN IMMEDIATE`
    /// instead would hold a connection idle, and a burst of writes would
    /// take every connection from the readers.
    pub write_queue: Arc<tokio::sync::Mutex<()>>,
    /// Each request's method and route, such as `GET /v1/issues/{issue}`,
    /// while a test records them. None, and free, otherwise.
    pub route_log: std::sync::Mutex<Option<Vec<String>>>,
}

impl std::ops::Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl Inner {
    pub fn pool(&self) -> SqlitePool {
        self.pool_slot.read().expect("pool lock").clone()
    }

    pub fn primary(&self) -> bool {
        self.is_primary.load(Ordering::SeqCst)
    }
}

/// The caller of one request: their context and their resolved actor.
pub struct Ctx {
    pub client: ClientContext,
    pub actor: Actor,
}

impl Ctx {
    pub fn actor_id(&self) -> String {
        self.actor.id.to_string()
    }

    pub fn session(&self) -> Option<&str> {
        self.client.session_id.as_deref().filter(|s| !s.is_empty())
    }
}

impl<S> FromRequestParts<S> for Ctx
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);
        let client = match parts.headers.get(CONTEXT_HEADER) {
            None => ClientContext::default(),
            Some(v) => {
                let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(v.as_bytes())
                    .map_err(|e| {
                        ApiError::bad_request(format!("{CONTEXT_HEADER} is not base64url: {e}"))
                    })?;
                serde_json::from_slice(&raw).map_err(|e| {
                    ApiError::bad_request(format!("{CONTEXT_HEADER} is not valid JSON: {e}"))
                })?
            }
        };
        if let Some(head) = &client.head
            && !is_commit_hash(head)
        {
            return Err(ApiError::bad_request(format!(
                "{CONTEXT_HEADER}: head must be a 7 to 64 character hex commit hash"
            )));
        }
        let actor = store::actors::resolve_input(&state.pool(), &client.actor).await?;
        Ok(Ctx { client, actor })
    }
}

/// True for a 7 to 64 character hex string, the only form a git commit
/// hash takes. The CLI passes stored heads to `git log`, so nothing else may
/// get in.
pub fn is_commit_hash(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// One write transaction. It records activity events and the issues it
/// touched, then on commit updates the work log and sends live events.
pub struct Write<'a> {
    pub tx: Transaction<'static, Sqlite>,
    pub ctx: &'a Ctx,
    state: AppState,
    seqs: Vec<i64>,
    /// Issues in the order the write touched them. The first one gets the
    /// write's token usage.
    touched: Vec<String>,
    /// False when the write changed only bookkeeping, such as a GitHub
    /// poll's poll time. Commit then leaves `sync_dirty` alone.
    count_for_sync: bool,
    /// This write's place at the head of `write_queue`. Dropped right after
    /// the transaction ends.
    queue: OwnedMutexGuard<()>,
    /// Dropped after the transaction ends.
    _guard: OwnedRwLockReadGuard<()>,
}

impl<'a> Write<'a> {
    pub async fn begin(state: &AppState, ctx: &'a Ctx) -> ApiResult<Self> {
        // Take the guard before the check, so a pull cannot start between them.
        let guard = state.writes.clone().read_owned().await;
        if !state.primary() {
            return Err(ApiError::new(
                ErrorCode::NotPrimary,
                "this machine is not the primary copy; run `llmtodo sync pull` to take over",
            ));
        }
        let queue = state.write_queue.clone().lock_owned().await;
        let tx = state.pool().begin_with("BEGIN IMMEDIATE").await?;
        Ok(Self {
            tx,
            ctx,
            state: state.clone(),
            seqs: Vec::new(),
            touched: Vec::new(),
            count_for_sync: true,
            queue,
            _guard: guard,
        })
    }

    /// Marks this write as bookkeeping only, so a sync push need not carry it.
    pub fn skip_sync_count(&mut self) {
        self.count_for_sync = false;
    }

    /// Rows changed on this write's connection so far. Compare two readings
    /// to learn whether a step changed anything.
    pub async fn changes(&mut self) -> ApiResult<i64> {
        store::settings::total_changes(&mut self.tx).await
    }

    /// The connection, for queries.
    pub fn c(&mut self) -> &mut sqlx::SqliteConnection {
        &mut self.tx
    }

    /// Records one activity event. `issue_id` ties it to an issue's log.
    pub async fn event(
        &mut self,
        entity_type: EntityType,
        entity_id: &str,
        issue_id: Option<&str>,
        field: ActivityField,
        old: Option<serde_json::Value>,
        new: Option<serde_json::Value>,
    ) -> ApiResult<()> {
        let old = old.map(|v| v.to_string());
        let new = new.map(|v| v.to_string());
        let actor = self.ctx.actor_id();
        let session = self.ctx.session().map(str::to_string);
        let seq: i64 = store::activity::insert(
            &mut self.tx,
            entity_type.as_str(),
            entity_id,
            issue_id,
            field.as_str(),
            old,
            new,
            actor,
            session,
        )
        .await?;
        self.seqs.push(seq);
        if let Some(id) = issue_id {
            self.touch(id);
        }
        Ok(())
    }

    /// Marks an issue as touched, so the work log records it.
    pub fn touch(&mut self, issue_id: &str) {
        if !self.touched.iter().any(|t| t == issue_id) {
            self.touched.push(issue_id.to_string());
        }
    }

    pub async fn commit(mut self) -> ApiResult<()> {
        // A write by the analyzing session keeps its mark alive. A mark
        // that already expired stays expired.
        if let Some(session) = self.ctx.session().map(str::to_string)
            && !self.touched.is_empty()
        {
            let t = crate::db::now();
            store::analyzing::refresh(
                &mut self.tx,
                &t,
                &session,
                store::issues::analyzing_cutoff(),
                serde_json::to_string(&self.touched)?,
            )
            .await?;
        }
        if self.ctx.actor.kind == ActorKind::Agent
            && let Some(session) = self.ctx.session().map(str::to_string)
        {
            let touched = std::mem::take(&mut self.touched);
            for issue_id in &touched {
                store::sessions::touch(&mut self.tx, self.ctx, &session, issue_id).await?;
            }
            if let Some(first) = touched.first() {
                store::usage::attribute_time(&mut self.tx, self.ctx, &session, first).await?;
                store::usage::attribute(&mut self.tx, self.ctx, &session, first).await?;
            }
        }
        // Counts writes since the last sync push. Push clears it only when
        // the count still equals the one inside its copy.
        if self.count_for_sync {
            store::settings::bump_sync_dirty(&mut self.tx).await?;
        }
        self.tx.commit().await?;
        // The next writer may start now. Sending events needs no write.
        drop(self.queue);
        if let Some(&mine) = self.seqs.iter().max() {
            // Writes commit in seq order, because each holds the write lock.
            // They can reach this point out of order, so each one sends
            // every committed event after the last one sent, including
            // those of an earlier write that has not sent yet.
            let mut sent = self.state.sent_seq.lock().await;
            if self.state.events.receiver_count() > 0 {
                send_after(&self.state, &mut sent).await;
            }
            *sent = (*sent).max(mine);
        }
        Ok(())
    }
}

/// Sends the committed events after `*sent` in seq order, and moves `*sent`
/// to the last one sent.
async fn send_after(state: &AppState, sent: &mut i64) {
    const BATCH: i64 = 500;
    let Ok(mut conn) = state.pool().acquire().await else {
        return;
    };
    loop {
        let Ok(events) = store::activity::after(&mut conn, *sent, None, BATCH).await else {
            return;
        };
        let n = events.len() as i64;
        for event in events {
            *sent = event.seq;
            let _ = state.events.send(WsMessage::Event {
                event: Box::new(event),
            });
        }
        if n < BATCH {
            return;
        }
    }
}

pub fn send_reload(state: &AppState) {
    let _ = state.events.send(WsMessage::Reload);
}
