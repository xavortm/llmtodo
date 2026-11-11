//! Analyzing.

use crate::error::ApiResult;
use sqlx::SqliteConnection;

pub async fn remove(
    c: &mut SqliteConnection,
    issue_id: &str,
) -> ApiResult<Option<(String, String)>> {
    Ok(
        sqlx::query_as("DELETE FROM analyzing WHERE issue_id = ? RETURNING session_id, seen_at")
            .bind(issue_id)
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn current(
    c: &mut SqliteConnection,
    issue_id: &str,
) -> ApiResult<Option<(String, String)>> {
    Ok(
        sqlx::query_as("SELECT session_id, seen_at FROM analyzing WHERE issue_id = ?")
            .bind(issue_id)
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn upsert(
    c: &mut SqliteConnection,
    issue_id: &str,
    actor_id: String,
    session: &str,
    t: &str,
    cutoff: String,
) -> ApiResult<()> {
    sqlx::query(
            "INSERT INTO analyzing (issue_id, actor_id, session_id, started_at, seen_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (issue_id) DO UPDATE SET
                started_at = CASE WHEN analyzing.session_id = excluded.session_id AND analyzing.seen_at >= ? THEN analyzing.started_at ELSE excluded.started_at END,
                actor_id = excluded.actor_id, session_id = excluded.session_id, seen_at = excluded.seen_at",
        )
        .bind(issue_id)
        .bind(actor_id)
        .bind(session)
        .bind(t)
        .bind(t)
        .bind(cutoff)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn refresh(
    c: &mut SqliteConnection,
    seen_at: &str,
    session: &str,
    cutoff: String,
    touched: String,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE analyzing SET seen_at = ? WHERE session_id = ? AND seen_at >= ?
                 AND issue_id IN (SELECT value FROM json_each(?))",
    )
    .bind(seen_at)
    .bind(session)
    .bind(cutoff)
    .bind(touched)
    .execute(&mut *c)
    .await?;
    Ok(())
}
