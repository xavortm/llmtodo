//! Claims.

use crate::db::now;
use crate::error::ApiResult;
use sqlx::SqliteConnection;

pub async fn live(
    c: &mut SqliteConnection,
    issue_id: &str,
) -> ApiResult<Option<(String, String, String)>> {
    Ok(sqlx::query_as(
        "SELECT id, actor_id, session_id FROM claims WHERE issue_id = ? AND ended_at IS NULL",
    )
    .bind(issue_id)
    .fetch_optional(&mut *c)
    .await?)
}

/// The live claims on issues in a review status, as (claim, issue) pairs.
/// With `status_id`, only issues in that status.
pub async fn live_in_review(
    c: &mut SqliteConnection,
    status_id: Option<&str>,
) -> ApiResult<Vec<(String, String)>> {
    Ok(sqlx::query_as(
        "SELECT c.id, c.issue_id FROM claims c JOIN issues i ON i.id = c.issue_id JOIN statuses s ON s.id = i.status_id
         WHERE c.ended_at IS NULL AND s.review = 1 AND (? IS NULL OR s.id = ?) ORDER BY c.created_at",
    )
    .bind(status_id)
    .bind(status_id)
    .fetch_all(&mut *c)
    .await?)
}

/// Whether the server still has to end the claims on issues in a review
/// status, after migration 0012.
pub async fn review_sweep_pending(c: &mut SqliteConnection) -> ApiResult<bool> {
    let found: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(REVIEW_SWEEP_KEY)
        .fetch_optional(&mut *c)
        .await?;
    Ok(found.is_some())
}

pub async fn finish_review_sweep(c: &mut SqliteConnection) -> ApiResult<()> {
    sqlx::query("DELETE FROM settings WHERE key = ?")
        .bind(REVIEW_SWEEP_KEY)
        .execute(&mut *c)
        .await?;
    Ok(())
}

const REVIEW_SWEEP_KEY: &str = "review_claims_sweep";

pub async fn end(c: &mut SqliteConnection, reason: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE claims SET ended_at = ?, end_reason = ? WHERE id = ?")
        .bind(now())
        .bind(reason)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_actor(c: &mut SqliteConnection, actor_id: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE claims SET actor_id = ? WHERE id = ?")
        .bind(actor_id)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    c: &mut SqliteConnection,
    id: &str,
    issue_id: &str,
    actor_id: &str,
    session: &str,
    host: Option<&str>,
    worktree: Option<&str>,
    branch: Option<&str>,
    herdr_pane: Option<&str>,
    stolen_from: Option<&str>,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO claims (id, issue_id, actor_id, session_id, host, worktree, branch, herdr_pane, stolen_from, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
        .bind(id)
        .bind(issue_id)
        .bind(actor_id)
        .bind(session)
        .bind(host)
        .bind(worktree)
        .bind(branch)
        .bind(herdr_pane)
        .bind(stolen_from)
        .bind(now())
        .execute(&mut *c)
        .await?;
    Ok(())
}
