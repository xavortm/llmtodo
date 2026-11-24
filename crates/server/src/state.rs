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
use tokio::sync::{Notify, OwnedMutexGuard, OwnedRwLockReadGuard, RwLock, broadcast};

use crate::error::{ApiError, ApiResult};
use crate::store;
use crate::store::backend::{self, Conn, Pool};

#[derive(Clone)]
pub struct AppState(pub Arc<Inner>);

pub struct Inner {
    /// Replaced when `sync pull` swaps the database file.
    pub pool_slot: std::sync::RwLock<Pool>,
    /// This machine's name, as the sync manifest records it.
    pub host: String,
    /// The shared database's host, port and name, with no credentials.
    /// `None` on SQLite.
    pub database_host: Option<String>,
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
    /// When the last request came, in Unix seconds. On a shared database
    /// the background jobs pause while nobody uses this server, so a host
    /// such as Neon can suspend the database (LLM-465).
    pub last_request: std::sync::atomic::AtomicI64,
    /// The actor each caller resolved to, so a request skips the lookup.
    /// Actors are never changed or deleted, so an entry stays true until
    /// `sync pull` swaps the database (LLM-469).
    pub actor_cache: std::sync::Mutex<std::collections::HashMap<String, Actor>>,
    /// Offline mode on a shared database: the local copy, the outbox and
    /// the Postgres pool (LLM-471). `None` on SQLite, and on Postgres when
    /// the server keeps no copy.
    pub shared: Option<Arc<crate::offline::Shared>>,
    /// Builds the router, for replaying offline changes through it. Set by
    /// `lib.rs`, so nothing outside `routes` names the routes module.
    pub router: std::sync::OnceLock<fn(AppState) -> axum::Router>,
}

impl std::ops::Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl Inner {
    /// Notes a request, for [`Inner::idle`].
    pub fn note_request(&self) {
        self.last_request
            .store(Timestamp::now().as_second(), Ordering::Relaxed);
    }

    /// True when no client listens live and no request came for `quiet`.
    pub fn idle(&self, quiet: std::time::Duration) -> bool {
        let last = self.last_request.load(Ordering::Relaxed);
        self.events.receiver_count() == 0
            && Timestamp::now().as_second() - last >= quiet.as_secs() as i64
    }

    pub fn pool(&self) -> Pool {
        self.pool_slot.read().expect("pool lock").clone()
    }

    /// Swaps the database, as `sync pull` does, and forgets what the old
    /// one held.
    pub fn set_pool(&self, pool: Pool) {
        *self.pool_slot.write().expect("pool lock") = pool;
        self.actor_cache.lock().expect("actor cache lock").clear();
    }

    /// The actor a request names, from the cache when it was seen before.
    async fn resolve_actor(&self, input: &llmtodo_api_types::ActorInput) -> ApiResult<Actor> {
        let key = format!("{input:?}");
        if let Some(a) = self.actor_cache.lock().expect("actor cache lock").get(&key) {
            return Ok(a.clone());
        }
        let actor = store::actors::resolve_input(&self.pool(), input).await?;
        self.actor_cache
            .lock()
            .expect("actor cache lock")
            .insert(key, actor.clone());
        Ok(actor)
    }

    pub fn primary(&self) -> bool {
        self.is_primary.load(Ordering::SeqCst)
    }

    /// True while the server works on its offline copy (LLM-471).
    pub fn is_offline(&self) -> bool {
        self.shared.as_ref().is_some_and(|s| s.is_offline())
    }

    /// True on a shared database, online or offline.
    pub fn is_shared(&self) -> bool {
        self.shared.is_some() || self.pool().backend() == backend::Backend::Postgres
    }

    /// The Postgres pool, also while the server is offline and serves its
    /// copy. `None` on SQLite, and before the first connection.
    pub fn postgres_pool(&self) -> Option<Pool> {
        match &self.shared {
            Some(s) => s.pg(),
            None => {
                let p = self.pool();
                (p.backend() == backend::Backend::Postgres).then_some(p)
            }
        }
    }
}

impl AppState {
    /// A second state over `pg` that replays offline changes (LLM-471). It
    /// is a plain online server with no clients: its writes take their own
    /// locks, and its live events reach nobody. The real server reloads its
    /// clients after the replay.
    pub fn replay_twin(&self, pg: Pool) -> AppState {
        let (events, _) = broadcast::channel(16);
        let twin = AppState(Arc::new(Inner {
            pool_slot: std::sync::RwLock::new(pg),
            host: self.host.clone(),
            database_host: self.database_host.clone(),
            data_dir: self.data_dir.clone(),
            events,
            sent_seq: tokio::sync::Mutex::new(i64::MAX),
            started_at: self.started_at,
            shutdown: Notify::new(),
            is_primary: AtomicBool::new(true),
            writes: Default::default(),
            write_queue: Default::default(),
            route_log: Default::default(),
            last_request: std::sync::atomic::AtomicI64::new(Timestamp::now().as_second()),
            actor_cache: Default::default(),
            shared: None,
            router: std::sync::OnceLock::new(),
        }));
        if let Some(r) = self.router.get() {
            let _ = twin.router.set(*r);
        }
        twin
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
        state.note_request();
        let actor = state.resolve_actor(&client.actor).await?;
        Ok(Ctx { client, actor })
    }
}

/// True for a 7 to 64 character hex string, the only form a git commit
/// hash takes. The CLI passes stored heads to `git log`, so nothing else may
/// get in.
pub fn is_commit_hash(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The statuses, actors and teams a write reads, loaded once per write
/// (LLM-469).
pub struct Catalog {
    pub lookup: store::issues::Lookup,
    /// Every team, by key.
    pub teams: Vec<store::teams::TeamRow>,
    pub default_team: Option<String>,
}

impl Catalog {
    /// Loads the four parts at once on connections of `pool`. A write does
    /// this on Postgres while it holds the write lock, so no other write
    /// can change them meanwhile.
    async fn load_from(pool: Pool) -> ApiResult<Self> {
        use store::{actors, teams};
        let conn = || async { pool.acquire().await.map_err(ApiError::from) };
        let (statuses, actors, teams, default_team) = tokio::try_join!(
            async { teams::status_map(&mut conn().await?).await },
            async { actors::all(&mut conn().await?).await },
            async { teams::team_rows(&mut conn().await?).await },
            async { teams::default_team_setting(&mut conn().await?).await },
        )?;
        Ok(Self {
            lookup: store::issues::Lookup { statuses, actors },
            teams,
            default_team,
        })
    }

    /// Loads it on one connection, which sees what its transaction wrote.
    async fn load(c: &mut Conn) -> ApiResult<Self> {
        use store::{actors, teams};
        Ok(Self {
            lookup: store::issues::Lookup {
                statuses: teams::status_map(c).await?,
                actors: actors::all(c).await?,
            },
            teams: teams::team_rows(c).await?,
            default_team: teams::default_team_setting(c).await?,
        })
    }

    /// The statuses of one team, in list order.
    fn team_statuses(&self, team_id: &str) -> Vec<store::teams::StatusRow> {
        let mut v: Vec<_> = self
            .lookup
            .statuses
            .values()
            .filter(|s| s.team_id == team_id)
            .cloned()
            .collect();
        v.sort_by(|a, b| {
            a.position
                .cmp(&b.position)
                .then_with(|| a.name.cmp(&b.name))
        });
        v
    }
}

/// One write transaction. It records activity events and the issues it
/// touched, then on commit updates the work log and sends live events.
pub struct Write<'a> {
    pub tx: Conn,
    pub ctx: &'a Ctx,
    state: AppState,
    /// Events not inserted yet. Commit inserts them in one statement.
    pending: Vec<store::activity::NewEvent>,
    /// The highest seq inserted so far.
    max_seq: Option<i64>,
    /// Issues whose `updated_at` moves at commit.
    updated: Vec<String>,
    /// Inserts that run at commit, on Postgres. See [`Write::defer_insert`].
    deferred: Vec<backend::Query>,
    catalog: Option<std::sync::Arc<Catalog>>,
    /// On Postgres, the catalog starts to load as the write begins, beside
    /// the write's first statements, so a step that needs it seldom waits
    /// (LLM-469).
    prefetch: Option<tokio::task::JoinHandle<ApiResult<Catalog>>>,
    /// True once this write changed statuses, teams, actors or settings.
    /// Reads of them then run on this write's own connection, which sees
    /// the change.
    catalog_dirty: bool,
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

/// What a [`Write`] held when a savepoint opened.
pub struct Mark {
    pending: usize,
    max_seq: Option<i64>,
    updated: usize,
    deferred: usize,
    touched: usize,
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
        let tx = state.pool().begin_write().await?;
        let prefetch = tx
            .is_postgres()
            .then(|| tokio::spawn(Catalog::load_from(state.pool())));
        Ok(Self {
            tx,
            ctx,
            state: state.clone(),
            pending: Vec::new(),
            max_seq: None,
            updated: Vec::new(),
            deferred: Vec::new(),
            catalog: None,
            prefetch,
            catalog_dirty: false,
            touched: Vec::new(),
            count_for_sync: true,
            queue,
            _guard: guard,
        })
    }

    /// True while the server works on its offline copy. An issue created
    /// then gets a provisional key (LLM-471).
    pub fn offline(&self) -> bool {
        self.state.is_offline()
    }

    /// Marks this write as bookkeeping only, so a sync push need not carry it.
    pub fn skip_sync_count(&mut self) {
        self.count_for_sync = false;
    }

    /// Rows changed on this write's connection so far. Compare two readings
    /// to learn whether a step changed anything.
    pub async fn changes(&mut self) -> ApiResult<i64> {
        Ok(self.tx.changes() as i64)
    }

    /// The connection, for queries.
    pub fn c(&mut self) -> &mut crate::store::backend::Conn {
        &mut self.tx
    }

    /// Records one activity event. `issue_id` ties it to an issue's log.
    /// The row goes in at commit, or at [`Write::flush_events`].
    pub async fn event(
        &mut self,
        entity_type: EntityType,
        entity_id: &str,
        issue_id: Option<&str>,
        field: ActivityField,
        old: Option<serde_json::Value>,
        new: Option<serde_json::Value>,
    ) -> ApiResult<()> {
        self.pending.push(store::activity::NewEvent {
            id: crate::db::new_id(),
            entity_type: entity_type.as_str().to_string(),
            entity_id: entity_id.to_string(),
            issue_id: issue_id.map(str::to_string),
            field: field.as_str().to_string(),
            old: old.map(|v| v.to_string()),
            new: new.map(|v| v.to_string()),
            actor_id: self.ctx.actor_id(),
            session_id: self.ctx.session().map(str::to_string),
            created_at: crate::db::now(),
        });
        if let Some(id) = issue_id {
            self.touch(id);
        }
        Ok(())
    }

    /// Inserts the events recorded so far. A step that reads the activity
    /// table inside the write calls this first.
    pub async fn flush_events(&mut self) -> ApiResult<()> {
        let events = std::mem::take(&mut self.pending);
        let max = store::activity::insert_many(&mut self.tx, &events).await?;
        self.max_seq = self.max_seq.max(max);
        Ok(())
    }

    /// Runs an insert that returns nothing. On Postgres it waits for the
    /// statement that ends the write, so it costs no round trip of its own
    /// (LLM-469). Only an insert of a row that no later step of the write
    /// reads may wait. SQLite runs it at once.
    pub async fn defer_insert(&mut self, q: backend::Query) -> ApiResult<()> {
        if self.tx.is_postgres() {
            self.deferred.push(q);
        } else {
            q.execute(&mut self.tx).await?;
        }
        Ok(())
    }

    /// Opens a savepoint, so one step of a batch can fail and leave the
    /// others in place (LLM-518). End it with [`Write::release`] or
    /// [`Write::rollback_to`].
    pub async fn savepoint(&mut self) -> ApiResult<Mark> {
        backend::query("SAVEPOINT step")
            .execute(&mut self.tx)
            .await?;
        Ok(Mark {
            pending: self.pending.len(),
            max_seq: self.max_seq,
            updated: self.updated.len(),
            deferred: self.deferred.len(),
            touched: self.touched.len(),
        })
    }

    /// Keeps what the step since [`Write::savepoint`] did.
    pub async fn release(&mut self) -> ApiResult<()> {
        backend::query("RELEASE SAVEPOINT step")
            .execute(&mut self.tx)
            .await?;
        Ok(())
    }

    /// Undoes the step since `mark`: its rows, and the events, touches and
    /// deferred inserts it recorded.
    pub async fn rollback_to(&mut self, mark: Mark) -> ApiResult<()> {
        backend::query("ROLLBACK TO SAVEPOINT step")
            .execute(&mut self.tx)
            .await?;
        backend::query("RELEASE SAVEPOINT step")
            .execute(&mut self.tx)
            .await?;
        self.pending.truncate(mark.pending);
        self.max_seq = mark.max_seq;
        self.updated.truncate(mark.updated);
        self.deferred.truncate(mark.deferred);
        self.touched.truncate(mark.touched);
        Ok(())
    }

    /// Moves the issue's `updated_at` at commit, and marks it touched.
    pub fn touch_updated(&mut self, issue_id: &str) {
        if !self.updated.iter().any(|t| t == issue_id) {
            self.updated.push(issue_id.to_string());
        }
        self.touch(issue_id);
    }

    /// Notes that this write changed statuses, teams, actors or the
    /// default team, so the catalog loads again from this write.
    pub fn catalog_changed(&mut self) {
        self.catalog = None;
        if let Some(p) = self.prefetch.take() {
            p.abort();
        }
        self.catalog_dirty = true;
    }

    /// The statuses, actors and teams, loaded once per write.
    pub async fn catalog(&mut self) -> ApiResult<std::sync::Arc<Catalog>> {
        if let Some(c) = &self.catalog {
            return Ok(c.clone());
        }
        self.reload_catalog().await
    }

    async fn reload_catalog(&mut self) -> ApiResult<std::sync::Arc<Catalog>> {
        let c = match self.prefetch.take() {
            Some(p) => p
                .await
                .map_err(|e| ApiError::internal(format!("load the catalog: {e}")))??,
            None if self.tx.is_postgres() && !self.catalog_dirty => {
                Catalog::load_from(self.state.pool()).await?
            }
            None => Catalog::load(&mut self.tx).await?,
        };
        let c = std::sync::Arc::new(c);
        self.catalog = Some(c.clone());
        Ok(c)
    }

    /// The catalog, loaded again when it lacks something a step needs.
    /// Another request may have added an actor since it loaded, and a
    /// step of this write may have added a status.
    async fn catalog_with(
        &mut self,
        has: impl Fn(&Catalog) -> bool,
    ) -> ApiResult<std::sync::Arc<Catalog>> {
        let c = self.catalog().await?;
        if has(&c) {
            return Ok(c);
        }
        self.reload_catalog().await
    }

    /// The catalog as it stands, for the response after commit. None when
    /// this write changed it or never loaded it.
    pub async fn catalog_for_response(&mut self) -> Option<std::sync::Arc<Catalog>> {
        if self.catalog_dirty {
            return None;
        }
        if self.catalog.is_none() && self.prefetch.is_some() {
            return self.catalog().await.ok();
        }
        self.catalog.clone()
    }

    /// Every actor, with each of `ids` in it when it exists.
    pub async fn actors(&mut self, ids: &[&str]) -> ApiResult<std::sync::Arc<Catalog>> {
        self.catalog_with(|c| ids.iter().all(|id| c.lookup.actors.contains_key(*id)))
            .await
    }

    pub async fn status(&mut self, id: &str) -> ApiResult<store::teams::StatusRow> {
        self.catalog_with(|c| c.lookup.statuses.contains_key(id))
            .await?
            .lookup
            .statuses
            .get(id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("status", id))
    }

    /// The statuses of one team, in list order.
    pub async fn team_statuses(
        &mut self,
        team_id: &str,
    ) -> ApiResult<Vec<store::teams::StatusRow>> {
        let c = self
            .catalog_with(|c| c.lookup.statuses.values().any(|s| s.team_id == team_id))
            .await?;
        Ok(c.team_statuses(team_id))
    }

    /// A status of one team by name or ID, as `teams::status_by_name`.
    pub async fn status_by_name(
        &mut self,
        team_id: &str,
        r: &str,
    ) -> ApiResult<store::teams::StatusRow> {
        let r = r.trim();
        let find = |c: &Catalog| {
            c.team_statuses(team_id)
                .into_iter()
                .find(|s| s.id == r || s.name.eq_ignore_ascii_case(r))
        };
        let c = self.catalog_with(|c| find(c).is_some()).await?;
        find(&c).ok_or_else(|| ApiError::not_found("status", r))
    }

    /// The first status of a type, as `teams::first_of_type`.
    pub async fn first_of_type(
        &mut self,
        team_id: &str,
        kind: llmtodo_api_types::StatusType,
    ) -> ApiResult<Option<store::teams::StatusRow>> {
        Ok(self
            .team_statuses(team_id)
            .await?
            .into_iter()
            .find(|s| s.kind() == kind))
    }

    /// The status a claim moves an issue to, as `teams::first_work_status`.
    pub async fn first_work_status(
        &mut self,
        team_id: &str,
    ) -> ApiResult<Option<store::teams::StatusRow>> {
        Ok(self
            .team_statuses(team_id)
            .await?
            .into_iter()
            .find(|s| s.kind() == llmtodo_api_types::StatusType::Started && !s.review))
    }

    /// A team by key or ID, as `teams::team_row`.
    pub async fn team(&mut self, r: &str) -> ApiResult<store::teams::TeamRow> {
        let c = self
            .catalog_with(|c| store::teams::find_team(&c.teams, r).is_ok())
            .await?;
        store::teams::find_team(&c.teams, r).cloned()
    }

    /// The default team, as `teams::default_team`.
    pub async fn default_team(&mut self) -> ApiResult<store::teams::TeamRow> {
        let c = self.catalog().await?;
        store::teams::pick_default_team(c.default_team.clone(), c.teams.clone())
    }

    /// Marks an issue as touched, so the work log records it.
    pub fn touch(&mut self, issue_id: &str) {
        if !self.touched.iter().any(|t| t == issue_id) {
            self.touched.push(issue_id.to_string());
        }
    }

    pub async fn commit(mut self) -> ApiResult<()> {
        // The statements that end every write. On Postgres the ones that
        // need nothing back run as one statement with the activity insert
        // and the notice, so they cost one round trip (LLM-469). SQLite
        // costs no round trip, so it runs them one by one.
        let pg = self.tx.is_postgres();
        let mut parts: Vec<backend::Query> = std::mem::take(&mut self.deferred);
        if !self.updated.is_empty() {
            parts.push(store::issues::touch_updated_query(
                &crate::db::now(),
                &self.updated,
            )?);
        }
        // A write by the analyzing session keeps its mark alive. A mark
        // that already expired stays expired.
        if let Some(session) = self.ctx.session().map(str::to_string)
            && !self.touched.is_empty()
        {
            parts.push(store::analyzing::refresh_query(
                &crate::db::now(),
                &session,
                store::issues::analyzing_cutoff(),
                serde_json::to_string(&self.touched)?,
            ));
        }
        if self.ctx.actor.kind == ActorKind::Agent
            && let Some(session) = self.ctx.session().map(str::to_string)
            && !self.touched.is_empty()
        {
            let touched = std::mem::take(&mut self.touched);
            let touch = store::sessions::touch_query(self.ctx, &session, &touched);
            // Usage needs the work session row, so it waits for the touch.
            if store::usage::reported(self.ctx) || !pg {
                touch.execute(&mut self.tx).await?;
                let first = &touched[0];
                store::usage::attribute_time(&mut self.tx, self.ctx, &session, first).await?;
                store::usage::attribute(&mut self.tx, self.ctx, &session, first).await?;
            } else {
                parts.push(touch);
            }
        }
        // Counts writes since the last sync push. Push clears it only when
        // the count still equals the one inside its copy.
        if self.count_for_sync && !pg {
            store::settings::bump_sync_dirty(&mut self.tx).await?;
        }
        if pg {
            // Other servers on a shared database hear of the write on
            // commit, and send its events to their own clients (LLM-459).
            let mut events = std::mem::take(&mut self.pending);
            let last = events.len().saturating_sub(1) / store::activity::INSERT_BATCH
                * store::activity::INSERT_BATCH;
            let tail = events.split_off(last);
            let early = store::activity::insert_many(&mut self.tx, &events).await?;
            let max = store::activity::finish_pg(&mut self.tx, parts, &tail).await?;
            self.max_seq = self.max_seq.max(early).max(max);
            if max.is_none()
                && let Some(seq) = self.max_seq
            {
                store::activity::notify(&mut self.tx, seq).await?;
            }
        } else {
            for part in parts {
                part.execute(&mut self.tx).await?;
            }
            self.flush_events().await?;
        }
        if let Err(e) = self.tx.commit().await {
            // The commit may have landed before the connection dropped, so
            // the request must not run again on the offline copy (LLM-471).
            let mut err = ApiError::from(e);
            if err.code == ErrorCode::DatabaseUnreachable {
                err.details["commit_unknown"] = serde_json::Value::Bool(true);
            }
            return Err(err);
        }
        // The next writer may start now. Sending events needs no write.
        drop(self.queue);
        if pg && let Some(shared) = &self.state.shared {
            shared.changed();
        }
        if let Some(mine) = self.max_seq {
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

/// Sends the events another server on the shared database committed. Each
/// write there notifies on commit. The write lock keeps seqs in commit
/// order, so events still go out in seq order with no gap. After a lost
/// listen connection the next notice catches up on everything missed.
///
/// It always reads from the last seq sent, even with no client listening.
/// Jumping to the newest seq instead would skip a local write that commits
/// during the read, and its client would never hear of it.
pub async fn send_remote(state: &AppState) {
    if let Some(shared) = &state.shared {
        shared.changed();
    }
    if state.is_offline() {
        return;
    }
    let mut sent = state.sent_seq.lock().await;
    send_after(state, &mut sent).await;
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
