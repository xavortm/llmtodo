//! The owner's review answers.

use llmtodo_api_types::{ReviewAnswer, ReviewVerdict};
use sqlx::SqliteConnection;

use crate::db::{new_id, now, parse_ts};
use crate::error::ApiResult;
use crate::store::actors;

#[derive(sqlx::FromRow)]
struct Row {
    round: i64,
    verdict: String,
    feedback: String,
    author_id: Option<String>,
    created_at: String,
}

/// Every answer of one issue, oldest first.
pub async fn for_issue(c: &mut SqliteConnection, issue_id: &str) -> ApiResult<Vec<ReviewAnswer>> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT round, verdict, feedback, author_id, created_at FROM reviews
         WHERE issue_id = ? ORDER BY round",
    )
    .bind(issue_id)
    .fetch_all(&mut *c)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let all = actors::all(c).await?;
    Ok(rows
        .into_iter()
        .map(|r| ReviewAnswer {
            round: r.round,
            verdict: r.verdict.parse().unwrap_or(ReviewVerdict::Approved),
            feedback: r.feedback,
            author: r.author_id.and_then(|a| all.get(&a).cloned()),
            created_at: parse_ts(&r.created_at),
        })
        .collect())
}

/// The highest round of an issue, or 0.
pub async fn last_round(c: &mut SqliteConnection, issue_id: &str) -> ApiResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT coalesce(max(round), 0) FROM reviews WHERE issue_id = ?")
            .bind(issue_id)
            .fetch_one(&mut *c)
            .await?,
    )
}

pub async fn insert(
    c: &mut SqliteConnection,
    issue_id: &str,
    round: i64,
    verdict: ReviewVerdict,
    feedback: &str,
    author_id: &str,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO reviews (id, issue_id, round, verdict, feedback, author_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(new_id())
    .bind(issue_id)
    .bind(round)
    .bind(verdict.as_str())
    .bind(feedback)
    .bind(author_id)
    .bind(now())
    .execute(&mut *c)
    .await?;
    Ok(())
}

/// The seq of the newest event that moved the issue into status `name`,
/// and of the newest review answer. Either is None when it never happened.
pub async fn entry_and_answer_seqs(
    c: &mut SqliteConnection,
    issue_id: &str,
    name: &str,
) -> ApiResult<(Option<i64>, Option<i64>)> {
    let status_json = serde_json::to_string(name)?;
    Ok(sqlx::query_as(
        "SELECT
            (SELECT max(seq) FROM activity WHERE issue_id = ?1 AND field = 'status' AND new = ?2),
            (SELECT max(seq) FROM activity WHERE issue_id = ?1 AND field = 'review')",
    )
    .bind(issue_id)
    .bind(status_json)
    .fetch_one(&mut *c)
    .await?)
}
