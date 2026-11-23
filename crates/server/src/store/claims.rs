//! Claims.

use crate::db::now;
use crate::error::ApiResult;
use crate::store::backend;
use crate::store::backend::Conn;

pub async fn live(c: &mut Conn, issue_id: &str) -> ApiResult<Option<(String, String, String)>> {
    Ok(backend::query_as(
        "SELECT id, actor_id, session_id FROM claims WHERE issue_id = ? AND ended_at IS NULL",
    )
    .bind(issue_id)
    .fetch_optional(&mut *c)
    .await?)
}

/// The live claim on an issue, as [`live`] gives it, and whether the issue
/// has an Analyzing mark, in one statement for a claim (LLM-469).
pub async fn live_and_marked(
    c: &mut Conn,
    issue_id: &str,
) -> ApiResult<(Option<(String, String, String)>, bool)> {
    let (id, actor, session, marks): (Option<String>, Option<String>, Option<String>, i64) =
        backend::query_as(
            "SELECT c.id, c.actor_id, c.session_id,
                CAST((SELECT count(*) FROM analyzing a WHERE a.issue_id = ?) AS BIGINT)
             FROM (SELECT 1 AS one) base
             LEFT JOIN claims c ON c.issue_id = ? AND c.ended_at IS NULL",
        )
        .bind(issue_id)
        .bind(issue_id)
        .fetch_one(&mut *c)
        .await?;
    let live = match (id, actor, session) {
        (Some(id), Some(actor), Some(session)) => Some((id, actor, session)),
        _ => None,
    };
    Ok((live, marks > 0))
}

/// The live claims on issues in a review status, as (claim, issue) pairs.
/// With `status_id`, only issues in that status.
pub async fn live_in_review(
    c: &mut Conn,
    status_id: Option<&str>,
) -> ApiResult<Vec<(String, String)>> {
    Ok(backend::query_as(
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
pub async fn review_sweep_pending(c: &mut Conn) -> ApiResult<bool> {
    let found: Option<String> = backend::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(REVIEW_SWEEP_KEY)
        .fetch_optional(&mut *c)
        .await?;
    Ok(found.is_some())
}

pub async fn finish_review_sweep(c: &mut Conn) -> ApiResult<()> {
    backend::query("DELETE FROM settings WHERE key = ?")
        .bind(REVIEW_SWEEP_KEY)
        .execute(&mut *c)
        .await?;
    Ok(())
}

const REVIEW_SWEEP_KEY: &str = "review_claims_sweep";

pub async fn end(c: &mut Conn, reason: &str, id: &str) -> ApiResult<()> {
    backend::query("UPDATE claims SET ended_at = ?, end_reason = ? WHERE id = ?")
        .bind(now())
        .bind(reason)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_actor(c: &mut Conn, actor_id: &str, id: &str) -> ApiResult<()> {
    backend::query("UPDATE claims SET actor_id = ? WHERE id = ?")
        .bind(actor_id)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    c: &mut Conn,
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
    backend::query(
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
