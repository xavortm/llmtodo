//! Documents.

use llmtodo_api_types::Document;
use sqlx::SqliteConnection;

use crate::db::{now, parse_ts, parse_ts_opt, parse_uuid};
use crate::error::{ApiError, ApiResult};

#[derive(sqlx::FromRow)]
pub struct DocRow {
    pub id: String,
    pub project_id: String,
    pub project_slug: String,
    pub slug: String,
    pub title: String,
    pub body: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

impl DocRow {
    pub fn to_api(&self, with_body: bool) -> Document {
        Document {
            id: parse_uuid(&self.id),
            project: self.project_slug.clone(),
            slug: self.slug.clone(),
            title: self.title.clone(),
            body: with_body.then(|| self.body.clone()),
            version: self.version,
            created_at: parse_ts(&self.created_at),
            updated_at: parse_ts(&self.updated_at),
            deleted_at: parse_ts_opt(self.deleted_at.clone()),
        }
    }
}

const D_SELECT: &str = "SELECT d.id, d.project_id, p.slug AS project_slug, d.slug, d.title, d.body, d.version, d.created_at, d.updated_at, d.deleted_at
    FROM documents d JOIN projects p ON p.id = d.project_id";

pub async fn documents(
    conn: &mut SqliteConnection,
    project_id: Option<&str>,
    trashed: bool,
) -> ApiResult<Vec<DocRow>> {
    Ok(sqlx::query_as::<_, DocRow>(crate::sql!(
        "{D_SELECT} WHERE (? IS NULL OR d.project_id = ?) AND ((? AND d.deleted_at IS NOT NULL) OR (NOT ? AND d.deleted_at IS NULL)) ORDER BY p.slug, d.title COLLATE NOCASE"
    ))
    .bind(project_id)
    .bind(project_id)
    .bind(trashed)
    .bind(trashed)
    .fetch_all(&mut *conn)
    .await?)
}

/// Documents that match an FTS5 `terms` query, best match first.
pub async fn search_documents(
    conn: &mut SqliteConnection,
    project_id: Option<&str>,
    trashed: bool,
    terms: &str,
) -> ApiResult<Vec<DocRow>> {
    Ok(sqlx::query_as::<_, DocRow>(crate::sql!(
        "WITH fts AS MATERIALIZED (SELECT entity_id, -rank AS score FROM search_fts WHERE search_fts MATCH ? AND entity_type = 'document')
        {D_SELECT} JOIN fts ON fts.entity_id = d.id
        WHERE (? IS NULL OR d.project_id = ?) AND ((? AND d.deleted_at IS NOT NULL) OR (NOT ? AND d.deleted_at IS NULL))
        ORDER BY fts.score DESC, p.slug, d.title COLLATE NOCASE"
    ))
    .bind(terms)
    .bind(project_id)
    .bind(project_id)
    .bind(trashed)
    .bind(trashed)
    .fetch_all(&mut *conn)
    .await?)
}

/// Resolves a document by ID, or by `project/slug`.
pub async fn resolve(conn: &mut SqliteConnection, r: &str) -> ApiResult<DocRow> {
    let r = r.trim();
    let row = if let Some((project, slug)) = r.split_once('/') {
        sqlx::query_as::<_, DocRow>(crate::sql!(
            "{D_SELECT} WHERE (p.slug = ? COLLATE NOCASE OR p.id = ?) AND d.slug = ? COLLATE NOCASE"
        ))
        .bind(project)
        .bind(project)
        .bind(slug)
        .fetch_optional(&mut *conn)
        .await?
    } else {
        sqlx::query_as::<_, DocRow>(crate::sql!("{D_SELECT} WHERE d.id = ?"))
            .bind(r)
            .fetch_optional(&mut *conn)
            .await?
    };
    row.ok_or_else(|| ApiError::not_found("document", r))
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    c: &mut SqliteConnection,
    id: &str,
    project_id: &str,
    slug: &str,
    title: &str,
    body: &str,
    actor_id: String,
    t: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO documents (id, project_id, slug, title, body, created_by, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(id)
        .bind(project_id)
        .bind(slug)
        .bind(title)
        .bind(body)
        .bind(actor_id)
        .bind(t)
        .bind(t)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_title(c: &mut SqliteConnection, title: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE documents SET title = ? WHERE id = ?")
        .bind(title)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_body(
    c: &mut SqliteConnection,
    body: &str,
    version: i64,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE documents SET body = ?, version = ? WHERE id = ?")
        .bind(body)
        .bind(version)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn touch(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE documents SET updated_at = ? WHERE id = ?")
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn trash(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE documents SET deleted_at = ? WHERE id = ?")
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn untrash(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE documents SET deleted_at = NULL, updated_at = ? WHERE id = ?")
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn restore_body(
    c: &mut SqliteConnection,
    body: &str,
    version: i64,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE documents SET body = ?, version = ?, updated_at = ? WHERE id = ?")
        .bind(body)
        .bind(version)
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}
