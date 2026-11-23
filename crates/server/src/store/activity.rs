use crate::store::backend::{Conn, Pool};
use llmtodo_api_types::{ActivityEvent, ActivityField, EntityType};

use crate::db::{parse_ts, parse_uuid};
use crate::error::ApiResult;
use crate::store::actors;
use crate::store::backend;

#[derive(sqlx::FromRow)]
struct Row {
    seq: i64,
    entity_type: String,
    entity_id: String,
    issue_key: Option<String>,
    field: String,
    old: Option<String>,
    new: Option<String>,
    actor_id: Option<String>,
    session_id: Option<String>,
    created_at: String,
}

const SELECT: &str = "SELECT a.seq, a.entity_type, a.entity_id, i.key AS issue_key, a.field, a.old, a.new, a.actor_id, a.session_id, a.created_at
    FROM activity a LEFT JOIN issues i ON i.id = a.issue_id";

async fn build(conn: &mut Conn, rows: Vec<Row>) -> ApiResult<Vec<ActivityEvent>> {
    let actors = actors::all(conn).await?;
    let parse = |s: Option<String>| s.and_then(|s| serde_json::from_str(&s).ok());
    Ok(rows
        .into_iter()
        .map(|r| ActivityEvent {
            seq: r.seq,
            entity_type: EntityType::parse(&r.entity_type),
            entity_id: parse_uuid(&r.entity_id),
            issue: r.issue_key.and_then(|k| k.parse().ok()),
            field: ActivityField::parse(&r.field),
            old: parse(r.old),
            new: parse(r.new),
            actor: r.actor_id.and_then(|a| actors.get(&a).cloned()),
            session_id: r.session_id,
            created_at: parse_ts(&r.created_at),
        })
        .collect())
}

pub async fn by_seqs(pool: &Pool, seqs: &[i64]) -> ApiResult<Vec<ActivityEvent>> {
    let mut conn = pool.acquire().await?;
    let rows = backend::query_as::<Row>(crate::sql!(
        "{SELECT} WHERE a.seq IN (SELECT value FROM json_each(?)) ORDER BY a.seq"
    ))
    .bind(serde_json::to_string(seqs)?)
    .fetch_all(&mut conn)
    .await?;
    build(&mut conn, rows).await
}

/// The newest activity seq, or 0 when there is none.
pub async fn max_seq(pool: &Pool) -> ApiResult<i64> {
    Ok(
        backend::query_scalar("SELECT coalesce(max(seq), 0) FROM activity")
            .fetch_one(pool)
            .await?,
    )
}

pub async fn after(
    conn: &mut Conn,
    after_seq: i64,
    issue_id: Option<&str>,
    limit: i64,
) -> ApiResult<Vec<ActivityEvent>> {
    let rows = backend::query_as::<Row>(crate::sql!(
        "{SELECT} WHERE a.seq > ? AND (? IS NULL OR a.issue_id = ?) ORDER BY a.seq LIMIT ?"
    ))
    .bind(after_seq)
    .bind(issue_id)
    .bind(issue_id)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?;
    build(conn, rows).await
}

pub async fn for_issue(conn: &mut Conn, issue_id: &str) -> ApiResult<Vec<ActivityEvent>> {
    let rows =
        backend::query_as::<Row>(crate::sql!("{SELECT} WHERE a.issue_id = ? ORDER BY a.seq"))
            .bind(issue_id)
            .fetch_all(&mut *conn)
            .await?;
    build(conn, rows).await
}

pub async fn for_entity(conn: &mut Conn, entity_id: &str) -> ApiResult<Vec<ActivityEvent>> {
    let rows =
        backend::query_as::<Row>(crate::sql!("{SELECT} WHERE a.entity_id = ? ORDER BY a.seq"))
            .bind(entity_id)
            .fetch_all(&mut *conn)
            .await?;
    build(conn, rows).await
}

pub async fn last_seq(conn: &mut Conn) -> ApiResult<i64> {
    Ok(
        backend::query_scalar("SELECT coalesce(max(seq), 0) FROM activity")
            .fetch_one(&mut *conn)
            .await?,
    )
}

pub async fn first_seq(conn: &mut Conn) -> ApiResult<i64> {
    Ok(
        backend::query_scalar("SELECT coalesce(min(seq), 0) FROM activity")
            .fetch_one(&mut *conn)
            .await?,
    )
}

pub async fn description_seq(
    c: &mut Conn,
    issue_id: &str,
    version: String,
) -> ApiResult<Option<i64>> {
    Ok(backend::query_scalar(
        "SELECT seq FROM activity WHERE issue_id = ? AND field = 'description' AND new = ?
         ORDER BY seq DESC LIMIT 1",
    )
    .bind(issue_id)
    .bind(version)
    .fetch_optional(&mut *c)
    .await?)
}

pub async fn later_write(
    c: &mut Conn,
    issue_id: &str,
    after: i64,
    actor_id: &str,
) -> ApiResult<Option<(String, String)>> {
    Ok(backend::query_as(
        "SELECT actor_id, field FROM activity
         WHERE issue_id = ? AND seq > ? AND actor_id IS NOT NULL AND actor_id != ?
         ORDER BY seq LIMIT 1",
    )
    .bind(issue_id)
    .bind(after)
    .bind(actor_id)
    .fetch_optional(&mut *c)
    .await?)
}

/// One activity event a write recorded, waiting for the insert at commit.
pub struct NewEvent {
    pub id: String,
    pub entity_type: String,
    pub entity_id: String,
    pub issue_id: Option<String>,
    pub field: String,
    pub old: Option<String>,
    pub new: Option<String>,
    pub actor_id: String,
    pub session_id: Option<String>,
    pub created_at: String,
}

/// Events per insert statement. Ten values each keeps a statement well
/// under the bound value limits of both backends.
pub const INSERT_BATCH: usize = 500;

/// One statement that inserts up to [`INSERT_BATCH`] events and returns
/// their seqs (LLM-469).
pub fn insert_query(events: &[NewEvent]) -> backend::Query {
    let mut b = backend::QueryBuilder::new(
        "INSERT INTO activity (id, entity_type, entity_id, issue_id, field, old, new, actor_id, session_id, created_at) VALUES ",
    );
    for (i, e) in events.iter().enumerate() {
        if i > 0 {
            b.push(", ");
        }
        b.push("(")
            .push_bind(e.id.as_str())
            .push(", ")
            .push_bind(e.entity_type.as_str())
            .push(", ")
            .push_bind(e.entity_id.as_str())
            .push(", ")
            .push_bind(e.issue_id.as_deref())
            .push(", ")
            .push_bind(e.field.as_str())
            .push(", ")
            .push_bind(e.old.as_deref())
            .push(", ")
            .push_bind(e.new.as_deref())
            .push(", ")
            .push_bind(e.actor_id.as_str())
            .push(", ")
            .push_bind(e.session_id.as_deref())
            .push(", ")
            .push_bind(e.created_at.as_str())
            .push(")");
    }
    b.push(" RETURNING seq");
    b.build()
}

/// Inserts events in batches. Returns the highest seq, or None when there
/// were none.
pub async fn insert_many(c: &mut Conn, events: &[NewEvent]) -> ApiResult<Option<i64>> {
    let mut max = None;
    for chunk in events.chunks(INSERT_BATCH) {
        let rows = insert_query(chunk).fetch_all(&mut *c).await?;
        for r in rows {
            max = max.max(Some(r.try_get::<i64>("seq")?));
        }
    }
    Ok(max)
}

/// Postgres only: runs `parts`, inserts `events` and sends the notice of
/// [`notify`], all in one statement, so the end of a write costs one round
/// trip (LLM-469). Each part is a statement that returns nothing, and runs
/// as its own `WITH` entry: Postgres runs every one exactly once. Returns
/// the highest seq inserted.
pub async fn finish_pg(
    c: &mut Conn,
    parts: Vec<backend::Query>,
    events: &[NewEvent],
) -> ApiResult<Option<i64>> {
    debug_assert!(c.is_postgres() && events.len() <= INSERT_BATCH);
    if parts.is_empty() && events.is_empty() {
        return Ok(None);
    }
    let mut b = backend::QueryBuilder::new("WITH ");
    for (i, part) in parts.into_iter().enumerate() {
        if i > 0 {
            b.push(", ");
        }
        b.push(format_args!("part{i} AS ("))
            .push_query(part)
            .push(")");
    }
    if events.is_empty() {
        b.push(" SELECT 1");
        b.build().execute(&mut *c).await?;
        return Ok(None);
    }
    if b.sql() != "WITH " {
        b.push(", ");
    }
    // The notice carries the highest seq. A function call in the select
    // list runs once for the one row.
    b.push("ins AS (")
        .push_query(insert_query(events))
        .push(") SELECT x.m, CAST(pg_notify(")
        .push_bind(CHANNEL)
        .push(", CAST(x.m AS TEXT)) AS TEXT) FROM (SELECT max(seq) AS m FROM ins) x");
    let (max, _): (i64, String) = b.build_query_as().fetch_one(&mut *c).await?;
    Ok(Some(max))
}

/// The channel that tells servers on a shared Postgres database of new
/// activity.
pub const CHANNEL: &str = "llmtodo_activity";

/// Tells every server listening on the shared database that activity up to
/// `seq` exists. Postgres delivers it when the transaction commits.
pub async fn notify(c: &mut Conn, seq: i64) -> ApiResult<()> {
    backend::query("SELECT pg_notify(?, ?)")
        .bind(CHANNEL)
        .bind(seq.to_string())
        .execute(&mut *c)
        .await?;
    Ok(())
}
