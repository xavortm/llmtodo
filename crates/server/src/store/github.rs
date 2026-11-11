//! Github.

use crate::error::ApiResult;
use sqlx::SqliteConnection;

pub async fn issues_claimed_on_branch(
    c: &mut SqliteConnection,
    branch: &str,
) -> ApiResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT issue_id FROM claims WHERE branch = ? AND ended_at IS NULL",
    )
    .bind(branch)
    .fetch_all(&mut *c)
    .await?)
}

pub async fn issues_with_attachment(c: &mut SqliteConnection, url: &str) -> ApiResult<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT DISTINCT issue_id FROM attachments WHERE url = ?")
            .bind(url)
            .fetch_all(&mut *c)
            .await?,
    )
}

pub async fn repos(
    c: &mut SqliteConnection,
) -> ApiResult<Vec<(String, String, Option<String>, Option<String>)>> {
    Ok(
        sqlx::query_as::<_, (String, String, Option<String>, Option<String>)>(
            "SELECT project_id, repo, etag, last_polled_at FROM project_repos ORDER BY repo",
        )
        .fetch_all(&mut *c)
        .await?,
    )
}

pub async fn attachment_count(c: &mut SqliteConnection, issue_id: &str) -> ApiResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM attachments WHERE issue_id = ?")
            .bind(issue_id)
            .fetch_one(&mut *c)
            .await?,
    )
}

pub async fn has_attachment(
    c: &mut SqliteConnection,
    issue_id: &str,
    url: &str,
) -> ApiResult<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT 1 FROM attachments WHERE issue_id = ? AND url = ?")
            .bind(issue_id)
            .bind(url)
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn save_poll(
    c: &mut SqliteConnection,
    etag: &Option<String>,
    started: &str,
    project_id: &str,
    repo: &str,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE project_repos SET etag = ?, last_polled_at = ? WHERE project_id = ? AND repo = ?",
    )
    .bind(etag)
    .bind(started)
    .bind(project_id)
    .bind(repo)
    .execute(&mut *c)
    .await?;
    Ok(())
}

pub async fn repo_count(c: &mut SqliteConnection) -> ApiResult<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM project_repos")
        .fetch_one(&mut *c)
        .await?)
}
