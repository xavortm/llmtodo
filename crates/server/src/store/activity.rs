use llmtodo_api_types::{ActivityEvent, ActivityField, EntityType};
use sqlx::{SqliteConnection, SqlitePool};

use crate::db::{new_id, now, parse_ts, parse_uuid};
use crate::error::ApiResult;
use crate::store::actors;

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

async fn build(conn: &mut SqliteConnection, rows: Vec<Row>) -> ApiResult<Vec<ActivityEvent>> {
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

pub async fn by_seqs(pool: &SqlitePool, seqs: &[i64]) -> ApiResult<Vec<ActivityEvent>> {
    let mut conn = pool.acquire().await?;
    let rows = sqlx::query_as::<_, Row>(crate::sql!(
        "{SELECT} WHERE a.seq IN (SELECT value FROM json_each(?)) ORDER BY a.seq"
    ))
    .bind(serde_json::to_string(seqs)?)
    .fetch_all(&mut *conn)
    .await?;
    build(&mut conn, rows).await
}

/// The newest activity seq, or 0 when there is none.
pub async fn max_seq(pool: &SqlitePool) -> ApiResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT coalesce(max(seq), 0) FROM activity")
            .fetch_one(pool)
            .await?,
    )
}

pub async fn after(
    conn: &mut SqliteConnection,
    after_seq: i64,
    issue_id: Option<&str>,
    limit: i64,
) -> ApiResult<Vec<ActivityEvent>> {
    let rows = sqlx::query_as::<_, Row>(crate::sql!(
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

pub async fn for_issue(
    conn: &mut SqliteConnection,
    issue_id: &str,
) -> ApiResult<Vec<ActivityEvent>> {
    let rows =
        sqlx::query_as::<_, Row>(crate::sql!("{SELECT} WHERE a.issue_id = ? ORDER BY a.seq"))
            .bind(issue_id)
            .fetch_all(&mut *conn)
            .await?;
    build(conn, rows).await
}

pub async fn for_entity(
    conn: &mut SqliteConnection,
    entity_id: &str,
) -> ApiResult<Vec<ActivityEvent>> {
    let rows =
        sqlx::query_as::<_, Row>(crate::sql!("{SELECT} WHERE a.entity_id = ? ORDER BY a.seq"))
            .bind(entity_id)
            .fetch_all(&mut *conn)
            .await?;
    build(conn, rows).await
}

pub async fn last_seq(conn: &mut SqliteConnection) -> ApiResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT coalesce(max(seq), 0) FROM activity")
            .fetch_one(&mut *conn)
            .await?,
    )
}

pub async fn first_seq(conn: &mut SqliteConnection) -> ApiResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT coalesce(min(seq), 0) FROM activity")
            .fetch_one(&mut *conn)
            .await?,
    )
}

pub async fn description_seq(
    c: &mut SqliteConnection,
    issue_id: &str,
    version: String,
) -> ApiResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT seq FROM activity WHERE issue_id = ? AND field = 'description' AND new = ?
         ORDER BY seq DESC LIMIT 1",
    )
    .bind(issue_id)
    .bind(version)
    .fetch_optional(&mut *c)
    .await?)
}

pub async fn later_write(
    c: &mut SqliteConnection,
    issue_id: &str,
    after: i64,
    actor_id: &str,
) -> ApiResult<Option<(String, String)>> {
    Ok(sqlx::query_as(
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

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    c: &mut SqliteConnection,
    entity_type: &str,
    entity_id: &str,
    issue_id: Option<&str>,
    field: &str,
    old: Option<String>,
    new: Option<String>,
    actor: String,
    session: Option<String>,
) -> ApiResult<i64> {
    Ok(sqlx::query_scalar(
            "INSERT INTO activity (id, entity_type, entity_id, issue_id, field, old, new, actor_id, session_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING seq",
        )
        .bind(new_id())
        .bind(entity_type)
        .bind(entity_id)
        .bind(issue_id)
        .bind(field)
        .bind(old)
        .bind(new)
        .bind(actor)
        .bind(session)
        .bind(now())
        .fetch_one(&mut *c)
        .await?)
}
