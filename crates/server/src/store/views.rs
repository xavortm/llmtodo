//! Saved views.

use crate::store::backend::Conn;
use llmtodo_api_types::SavedView;

use crate::db;
use crate::db::now;
use crate::error::{ApiError, ApiResult};
use crate::store::backend;

#[derive(sqlx::FromRow)]
pub struct ViewRow {
    pub id: String,
    pub name: String,
    pub query: String,
    pub created_at: String,
    pub updated_at: String,
}

impl ViewRow {
    pub fn to_api(self) -> ApiResult<SavedView> {
        let r = self;
        Ok(SavedView {
            id: db::parse_uuid(&r.id),
            name: r.name,
            query: serde_json::from_str(&r.query)?,
            created_at: db::parse_ts(&r.created_at),
            updated_at: db::parse_ts(&r.updated_at),
        })
    }
}

pub async fn row(c: &mut crate::store::backend::Conn, r: &str) -> ApiResult<ViewRow> {
    backend::query_as::<ViewRow>("SELECT id, name, query, created_at, updated_at FROM saved_views WHERE id = ? OR name = ? COLLATE NOCASE")
        .bind(r)
        .bind(r)
        .fetch_optional(&mut *c)
        .await?
        .ok_or_else(|| ApiError::not_found("view", r))
}

pub async fn stored_query(c: &mut Conn, name: &str) -> ApiResult<Option<String>> {
    Ok(backend::query_scalar(
        "SELECT query FROM saved_views WHERE name = ? COLLATE NOCASE OR id = ?",
    )
    .bind(name)
    .bind(name)
    .fetch_optional(&mut *c)
    .await?)
}

pub async fn list(c: &mut Conn) -> ApiResult<Vec<ViewRow>> {
    Ok(backend::query_as::<ViewRow>("SELECT id, name, query, created_at, updated_at FROM saved_views ORDER BY name COLLATE NOCASE")
        .fetch_all(&mut *c)
        .await?)
}

pub async fn insert(c: &mut Conn, id: &str, name: &str, query: String, t: &str) -> ApiResult<()> {
    backend::query(
        "INSERT INTO saved_views (id, name, query, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(name)
    .bind(query)
    .bind(t)
    .bind(t)
    .execute(&mut *c)
    .await?;
    Ok(())
}

pub async fn set_name(c: &mut Conn, name: &str, id: &str) -> ApiResult<()> {
    backend::query("UPDATE saved_views SET name = ? WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_query(c: &mut Conn, query: String, id: &str) -> ApiResult<()> {
    backend::query("UPDATE saved_views SET query = ? WHERE id = ?")
        .bind(query)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn touch(c: &mut Conn, id: &str) -> ApiResult<()> {
    backend::query("UPDATE saved_views SET updated_at = ? WHERE id = ?")
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn delete(c: &mut Conn, id: &str) -> ApiResult<()> {
    backend::query("DELETE FROM saved_views WHERE id = ?")
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}
