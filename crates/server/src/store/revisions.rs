//! Saved versions of issue, comment and document text.

use llmtodo_api_types::Revision;
use sqlx::SqliteConnection;

use crate::db::{new_id, now, parse_ts};
use crate::error::{ApiError, ApiResult};
use crate::store::actors;

pub async fn add_revision(
    conn: &mut SqliteConnection,
    entity_type: &str,
    entity_id: &str,
    version: i64,
    body: &str,
    actor_id: &str,
    session: Option<&str>,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO revisions (id, entity_type, entity_id, version, body, actor_id, session_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(new_id())
        .bind(entity_type)
        .bind(entity_id)
        .bind(version)
        .bind(body)
        .bind(actor_id)
        .bind(session)
        .bind(now())
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Every revision of an entity, oldest first. The text comes only with
/// `bodies`.
pub async fn revisions(
    conn: &mut SqliteConnection,
    entity_type: &str,
    entity_id: &str,
    bodies: bool,
) -> ApiResult<Vec<Revision>> {
    let actors = actors::all(conn).await?;
    type Row = (i64, Option<String>, Option<String>, Option<String>, String);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT version, CASE WHEN ? THEN body END, actor_id, session_id, created_at FROM revisions WHERE entity_type = ? AND entity_id = ? ORDER BY version",
    )
    .bind(bodies)
    .bind(entity_type)
    .bind(entity_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(version, body, actor, session_id, created)| Revision {
            version,
            body,
            actor: actor.and_then(|a| actors.get(&a).cloned()),
            session_id,
            created_at: parse_ts(&created),
        })
        .collect())
}

pub async fn revision_body(
    conn: &mut SqliteConnection,
    entity_type: &str,
    entity_id: &str,
    version: i64,
) -> ApiResult<String> {
    sqlx::query_scalar(
        "SELECT body FROM revisions WHERE entity_type = ? AND entity_id = ? AND version = ?",
    )
    .bind(entity_type)
    .bind(entity_id)
    .bind(version)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("revision", &version.to_string()))
}

pub async fn of_issue_oldest_first(
    c: &mut SqliteConnection,
    issue_id: &str,
) -> ApiResult<Vec<(i64, String, Option<String>)>> {
    Ok(sqlx::query_as(
        "SELECT version, body, actor_id FROM revisions
         WHERE entity_type = 'issue' AND entity_id = ? ORDER BY version",
    )
    .bind(issue_id)
    .fetch_all(&mut *c)
    .await?)
}

pub async fn delete_of_issue(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM revisions WHERE entity_type = 'issue' AND entity_id = ?")
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn delete_of_comments(c: &mut SqliteConnection, issue_id: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM revisions WHERE entity_type = 'comment' AND entity_id IN (SELECT id FROM comments WHERE issue_id = ?)")
        .bind(issue_id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn delete_old(c: &mut SqliteConnection, before: String) -> ApiResult<()> {
    sqlx::query(
            "DELETE FROM revisions WHERE created_at < ?
             AND version < (SELECT max(version) FROM revisions r2 WHERE r2.entity_type = revisions.entity_type AND r2.entity_id = revisions.entity_id)",
        )
        .bind(before)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn count_old(c: &mut SqliteConnection, before: String) -> ApiResult<i64> {
    Ok(sqlx::query_scalar(
            "SELECT count(*) FROM revisions r WHERE r.created_at < ?
             AND r.version < (SELECT max(version) FROM revisions r2 WHERE r2.entity_type = r.entity_type AND r2.entity_id = r.entity_id)",
        )
        .bind(before)
        .fetch_one(&mut *c)
        .await?)
}
