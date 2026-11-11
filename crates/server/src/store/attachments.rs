//! Links and files attached to issues.

use llmtodo_api_types::{Attachment, AttachmentKind};
use sqlx::SqliteConnection;

use crate::db::{now, parse_ts, parse_uuid};
use crate::error::{ApiError, ApiResult};

#[derive(sqlx::FromRow)]
struct AttachmentRow {
    id: String,
    kind: String,
    url: String,
    title: Option<String>,
    subtitle: Option<String>,
    source: String,
    metadata: String,
    created_at: String,
}

fn attachment(r: AttachmentRow) -> Attachment {
    Attachment {
        id: parse_uuid(&r.id),
        kind: r.kind.parse().unwrap_or(AttachmentKind::Link),
        url: r.url,
        title: r.title,
        subtitle: r.subtitle,
        source: r.source,
        metadata: serde_json::from_str(&r.metadata).unwrap_or(serde_json::Value::Null),
        created_at: parse_ts(&r.created_at),
    }
}

pub async fn attachments(
    conn: &mut SqliteConnection,
    issue_id: &str,
) -> ApiResult<Vec<Attachment>> {
    let rows = sqlx::query_as::<_, AttachmentRow>("SELECT id, kind, url, title, subtitle, source, metadata, created_at FROM attachments WHERE issue_id = ? ORDER BY created_at")
        .bind(issue_id)
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows.into_iter().map(attachment).collect())
}

pub async fn attachment_by_id(
    conn: &mut SqliteConnection,
    id: &str,
) -> ApiResult<(String, Attachment)> {
    let issue_id: String = sqlx::query_scalar("SELECT issue_id FROM attachments WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| ApiError::not_found("attachment", id))?;
    let r = sqlx::query_as::<_, AttachmentRow>("SELECT id, kind, url, title, subtitle, source, metadata, created_at FROM attachments WHERE id = ?")
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    Ok((issue_id, attachment(r)))
}

/// Guesses the kind of an attachment from its URL.
pub fn guess_kind(url: &str) -> AttachmentKind {
    let u = url.trim();
    if u.contains("github.com/") && u.contains("/pull/") {
        AttachmentKind::PullRequest
    } else if u.contains("github.com/") && u.contains("/commit/") {
        AttachmentKind::Commit
    } else if u.starts_with('/')
        || u.starts_with("~/")
        || u.starts_with("file://")
        || u.starts_with("./")
    {
        AttachmentKind::Path
    } else {
        AttachmentKind::Link
    }
}

pub async fn id_by_url(
    c: &mut SqliteConnection,
    issue_id: &str,
    url: &str,
) -> ApiResult<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM attachments WHERE issue_id = ? AND url = ?")
            .bind(issue_id)
            .bind(url)
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn update_title_metadata(
    c: &mut SqliteConnection,
    title: Option<&str>,
    metadata: String,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE attachments SET title = coalesce(?, title), metadata = CASE WHEN ? = '{}' THEN metadata ELSE ? END WHERE id = ?")
        .bind(title)
        .bind(&metadata)
        .bind(&metadata)
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
    kind: &str,
    url: &str,
    title: Option<&str>,
    source: &str,
    metadata: String,
    actor_id: String,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO attachments (id, issue_id, kind, url, title, source, metadata, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(id)
        .bind(issue_id)
        .bind(kind)
        .bind(url)
        .bind(title)
        .bind(source)
        .bind(metadata)
        .bind(actor_id)
        .bind(now())
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn delete(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM attachments WHERE id = ?")
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}
