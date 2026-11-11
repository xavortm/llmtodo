//! The derived `questions` table.

use std::collections::HashMap;

use sqlx::SqliteConnection;

use crate::error::ApiResult;

const BACKFILL_KEY: &str = "questions_backfill";

/// One stored question. `options` and `answered_question` hold JSON.
#[derive(sqlx::FromRow)]
pub struct QuestionRow {
    pub position: i64,
    pub number: i64,
    pub text: String,
    pub context: String,
    pub options: String,
    pub answer: Option<String>,
    pub answered_version: Option<i64>,
    pub answered_by: Option<String>,
    pub answered_question: Option<String>,
}

/// One question in a list, with the issue it sits on.
#[derive(sqlx::FromRow)]
pub struct ListRow {
    pub issue_created_at: String,
    pub issue_id: String,
    pub position: i64,
    pub key: String,
    pub title: String,
    pub waiting_on: Option<String>,
    pub number: i64,
    pub text: String,
    pub context: String,
    pub options: String,
    pub answer: Option<String>,
}

/// What a list returns. Without `issue`, it covers open issues only.
pub struct ListFilter<'a> {
    pub issue: Option<&'a str>,
    pub project: Option<&'a str>,
    pub include_answered: bool,
    /// The sort values of the last row on the page before.
    pub after: Option<(&'a str, &'a str, i64)>,
    pub limit: i64,
}

pub async fn of_issue(c: &mut SqliteConnection, issue_id: &str) -> ApiResult<Vec<QuestionRow>> {
    Ok(sqlx::query_as(
        "SELECT position, number, text, context, options, answer,
                answered_version, answered_by, answered_question
         FROM questions WHERE issue_id = ? ORDER BY position",
    )
    .bind(issue_id)
    .fetch_all(&mut *c)
    .await?)
}

/// Replaces every question of one issue.
pub async fn replace(
    c: &mut SqliteConnection,
    issue_id: &str,
    rows: &[QuestionRow],
) -> ApiResult<()> {
    sqlx::query("DELETE FROM questions WHERE issue_id = ?")
        .bind(issue_id)
        .execute(&mut *c)
        .await?;
    for r in rows {
        sqlx::query(
            "INSERT INTO questions (issue_id, position, number, text, context, options, answer,
                                    answered_version, answered_by, answered_question)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(issue_id)
        .bind(r.position)
        .bind(r.number)
        .bind(&r.text)
        .bind(&r.context)
        .bind(&r.options)
        .bind(&r.answer)
        .bind(r.answered_version)
        .bind(&r.answered_by)
        .bind(&r.answered_question)
        .execute(&mut *c)
        .await?;
    }
    Ok(())
}

/// How many open questions each issue holds. Issues with none are absent.
pub async fn open_counts(
    c: &mut SqliteConnection,
    ids: &[String],
) -> ApiResult<HashMap<String, u32>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT issue_id, count(*) FROM questions
         WHERE answer IS NULL AND issue_id IN (SELECT value FROM json_each(?))
         GROUP BY issue_id",
    )
    .bind(serde_json::to_string(ids)?)
    .fetch_all(&mut *c)
    .await?;
    Ok(rows.into_iter().map(|(id, n)| (id, n as u32)).collect())
}

pub async fn list(c: &mut SqliteConnection, f: &ListFilter<'_>) -> ApiResult<Vec<ListRow>> {
    let (after_created, after_issue, after_position) = match f.after {
        Some((created, issue, position)) => (Some(created), Some(issue), Some(position)),
        None => (None, None, None),
    };
    Ok(sqlx::query_as(
        "SELECT i.created_at AS issue_created_at, q.issue_id, q.position, i.key, i.title, i.waiting_on,
                q.number, q.text, q.context, q.options, q.answer
         FROM questions q
         JOIN issues i ON i.id = q.issue_id
         JOIN statuses s ON s.id = i.status_id
         WHERE (? OR q.answer IS NULL)
           AND (q.issue_id = ? OR (? IS NULL AND i.deleted_at IS NULL
                                   AND s.type NOT IN ('completed', 'canceled')))
           AND (? IS NULL OR i.project_id = ?)
           AND (? IS NULL OR (i.created_at, q.issue_id, q.position) > (?, ?, ?))
         ORDER BY i.created_at, q.issue_id, q.position
         LIMIT ?",
    )
    .bind(f.include_answered)
    .bind(f.issue)
    .bind(f.issue)
    .bind(f.project)
    .bind(f.project)
    .bind(after_created)
    .bind(after_created)
    .bind(after_issue)
    .bind(after_position)
    .bind(f.limit)
    .fetch_all(&mut *c)
    .await?)
}

pub async fn backfill_pending(c: &mut SqliteConnection) -> ApiResult<bool> {
    let found: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(BACKFILL_KEY)
        .fetch_optional(&mut *c)
        .await?;
    Ok(found.is_some())
}

pub async fn finish_backfill(c: &mut SqliteConnection) -> ApiResult<()> {
    sqlx::query("DELETE FROM settings WHERE key = ?")
        .bind(BACKFILL_KEY)
        .execute(&mut *c)
        .await?;
    Ok(())
}

/// Issues, live and trashed, whose description can hold the section. LIKE
/// ignores ASCII case, as the section title does.
pub async fn issues_to_backfill(c: &mut SqliteConnection) -> ApiResult<Vec<(String, String)>> {
    Ok(sqlx::query_as(
        "SELECT id, description FROM issues WHERE description LIKE '%owner questions%'",
    )
    .fetch_all(&mut *c)
    .await?)
}
