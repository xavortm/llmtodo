//! Documents.

use crate::store::backend::Conn;
use llmtodo_api_types::Document;

use crate::db::{now, parse_ts, parse_ts_opt, parse_uuid};
use crate::error::{ApiError, ApiResult};
use crate::store::backend;

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

/// The list columns: every column but the body, which a list never sends.
const D_LIST: &str = "SELECT d.id, d.project_id, p.slug AS project_slug, d.slug, d.title, '' AS body, d.version, d.created_at, d.updated_at, d.deleted_at
    FROM documents d JOIN projects p ON p.id = d.project_id";

/// Lists documents with an empty body.
pub async fn documents(
    conn: &mut Conn,
    project_id: Option<&str>,
    trashed: bool,
) -> ApiResult<Vec<DocRow>> {
    Ok(backend::query_as::<DocRow>(crate::sql!(
        "{D_LIST} WHERE (? IS NULL OR d.project_id = ?) AND ((? = 1 AND d.deleted_at IS NOT NULL) OR (? = 0 AND d.deleted_at IS NULL)) ORDER BY p.slug, d.title COLLATE NOCASE"
    ))
    .bind(project_id)
    .bind(project_id)
    .bind(trashed)
    .bind(trashed)
    .fetch_all(&mut *conn)
    .await?)
}

/// Documents that match a text search, best match first, with an empty
/// body.
pub async fn search_documents(
    conn: &mut Conn,
    project_id: Option<&str>,
    trashed: bool,
    search: &crate::store::issues::Search,
) -> ApiResult<Vec<DocRow>> {
    let mut b = backend::QueryBuilder::new("");
    crate::store::issues::push_text_matches(
        &mut b,
        conn.backend(),
        "entity_id",
        search,
        " AND entity_type = 'document'",
    );
    b.push(crate::sql!("{D_LIST} JOIN fts ON fts.entity_id = d.id WHERE (").as_str())
        .push_bind(project_id)
        .push(" IS NULL OR d.project_id = ")
        .push_bind(project_id)
        .push(") AND ((")
        .push_bind(trashed)
        .push(" = 1 AND d.deleted_at IS NOT NULL) OR (")
        .push_bind(trashed)
        .push(" = 0 AND d.deleted_at IS NULL)) ORDER BY fts.score DESC, p.slug, d.title COLLATE NOCASE");
    Ok(b.build_query_as::<DocRow>().fetch_all(&mut *conn).await?)
}

/// Resolves a document by ID, or by `project/slug`.
pub async fn resolve(conn: &mut Conn, r: &str) -> ApiResult<DocRow> {
    let r = r.trim();
    let row = if let Some((project, slug)) = r.split_once('/') {
        backend::query_as::<DocRow>(crate::sql!(
            "{D_SELECT} WHERE (p.slug = ? COLLATE NOCASE OR p.id = ?) AND d.slug = ? COLLATE NOCASE"
        ))
        .bind(project)
        .bind(project)
        .bind(slug)
        .fetch_optional(&mut *conn)
        .await?
    } else {
        backend::query_as::<DocRow>(crate::sql!("{D_SELECT} WHERE d.id = ?"))
            .bind(r)
            .fetch_optional(&mut *conn)
            .await?
    };
    row.ok_or_else(|| ApiError::not_found("document", r))
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    c: &mut Conn,
    id: &str,
    project_id: &str,
    slug: &str,
    title: &str,
    body: &str,
    actor_id: String,
    t: &str,
) -> ApiResult<()> {
    backend::query("INSERT INTO documents (id, project_id, slug, title, body, created_by, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
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

pub async fn set_title(c: &mut Conn, title: &str, id: &str) -> ApiResult<()> {
    backend::query("UPDATE documents SET title = ? WHERE id = ?")
        .bind(title)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_body(c: &mut Conn, body: &str, version: i64, id: &str) -> ApiResult<()> {
    backend::query("UPDATE documents SET body = ?, version = ? WHERE id = ?")
        .bind(body)
        .bind(version)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn touch(c: &mut Conn, id: &str) -> ApiResult<()> {
    backend::query("UPDATE documents SET updated_at = ? WHERE id = ?")
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn trash(c: &mut Conn, id: &str) -> ApiResult<()> {
    backend::query("UPDATE documents SET deleted_at = ? WHERE id = ?")
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn untrash(c: &mut Conn, id: &str) -> ApiResult<()> {
    backend::query("UPDATE documents SET deleted_at = NULL, updated_at = ? WHERE id = ?")
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn restore_body(c: &mut Conn, body: &str, version: i64, id: &str) -> ApiResult<()> {
    backend::query("UPDATE documents SET body = ?, version = ?, updated_at = ? WHERE id = ?")
        .bind(body)
        .bind(version)
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}
