//! Links and files attached to issues.

use crate::store::backend::Conn;
use llmtodo_api_types::{Attachment, AttachmentKind};

use crate::db::{now, parse_ts, parse_uuid};
use crate::error::{ApiError, ApiResult};
use crate::store::backend;

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

pub async fn attachments(conn: &mut Conn, issue_id: &str) -> ApiResult<Vec<Attachment>> {
    let rows = backend::query_as::<AttachmentRow>("SELECT id, kind, url, title, subtitle, source, metadata, created_at FROM attachments WHERE issue_id = ? ORDER BY created_at")
        .bind(issue_id)
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows.into_iter().map(attachment).collect())
}

pub async fn attachment_by_id(conn: &mut Conn, id: &str) -> ApiResult<(String, Attachment)> {
    let issue_id: String = backend::query_scalar("SELECT issue_id FROM attachments WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| ApiError::not_found("attachment", id))?;
    let r = backend::query_as::<AttachmentRow>("SELECT id, kind, url, title, subtitle, source, metadata, created_at FROM attachments WHERE id = ?")
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

pub async fn id_by_url(c: &mut Conn, issue_id: &str, url: &str) -> ApiResult<Option<String>> {
    Ok(
        backend::query_scalar("SELECT id FROM attachments WHERE issue_id = ? AND url = ?")
            .bind(issue_id)
            .bind(url)
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn update_title_metadata(
    c: &mut Conn,
    title: Option<&str>,
    metadata: String,
    id: &str,
) -> ApiResult<()> {
    // Skips the row when nothing differs, so a PR read again counts as no
    // change.
    backend::query(
        "UPDATE attachments SET title = coalesce(?1, title),
             metadata = CASE WHEN ?2 = '{}' THEN metadata ELSE ?2 END
         WHERE id = ?3
           AND (title IS NOT coalesce(?1, title)
                OR metadata IS NOT CASE WHEN ?2 = '{}' THEN metadata ELSE ?2 END)",
    )
    .bind(title)
    .bind(&metadata)
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
    kind: &str,
    url: &str,
    title: Option<&str>,
    source: &str,
    metadata: String,
    actor_id: String,
) -> ApiResult<()> {
    backend::query("INSERT INTO attachments (id, issue_id, kind, url, title, source, metadata, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
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

pub async fn delete(c: &mut Conn, id: &str) -> ApiResult<()> {
    backend::query("DELETE FROM attachments WHERE id = ?")
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}
