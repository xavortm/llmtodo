//! Comments on issues.

use std::collections::HashMap;

use crate::store::backend::Conn;
use llmtodo_api_types::{Actor, Comment};

use crate::db::{now, parse_ts, parse_uuid};
use crate::error::{ApiError, ApiResult};
use crate::store::actors;
use crate::store::backend;

#[derive(sqlx::FromRow)]
struct CommentRow {
    id: String,
    issue_key: String,
    author_id: Option<String>,
    body: String,
    version: i64,
    parent_id: Option<String>,
    created_at: String,
    updated_at: String,
}

fn comment(r: CommentRow, actors: &HashMap<String, Actor>) -> Option<Comment> {
    Some(Comment {
        id: parse_uuid(&r.id),
        issue: r.issue_key.parse().ok()?,
        author: r.author_id.and_then(|a| actors.get(&a).cloned()),
        body: r.body,
        version: r.version,
        parent_id: r.parent_id.map(|p| parse_uuid(&p)),
        created_at: parse_ts(&r.created_at),
        updated_at: parse_ts(&r.updated_at),
    })
}

const C_SELECT: &str = "SELECT c.id, i.key AS issue_key, c.author_id, c.body, c.version, c.parent_id, c.created_at, c.updated_at FROM comments c JOIN issues i ON i.id = c.issue_id";

pub async fn comments(
    conn: &mut Conn,
    issue_id: &str,
    actors: &HashMap<String, Actor>,
) -> ApiResult<Vec<Comment>> {
    let rows = backend::query_as::<CommentRow>(crate::sql!(
        "{C_SELECT} WHERE c.issue_id = ? AND c.deleted_at IS NULL ORDER BY c.created_at"
    ))
    .bind(issue_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| comment(r, actors))
        .collect())
}

pub async fn comment_by_id(conn: &mut Conn, id: &str) -> ApiResult<(String, Comment)> {
    let issue_id: String =
        backend::query_scalar("SELECT issue_id FROM comments WHERE id = ? AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| ApiError::not_found("comment", id))?;
    let actors = actors::all(conn).await?;
    let r = backend::query_as::<CommentRow>(crate::sql!("{C_SELECT} WHERE c.id = ?"))
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    Ok((
        issue_id,
        comment(r, &actors).ok_or_else(|| ApiError::internal("bad comment row"))?,
    ))
}

pub async fn insert(
    c: &mut Conn,
    id: &str,
    issue_id: &str,
    parent_id: Option<String>,
    author_id: String,
    body: &str,
    t: &str,
) -> ApiResult<()> {
    backend::query("INSERT INTO comments (id, issue_id, parent_id, author_id, body, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(id)
        .bind(issue_id)
        .bind(parent_id)
        .bind(author_id)
        .bind(body)
        .bind(t)
        .bind(t)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn update_body(c: &mut Conn, body: &str, version: i64, id: &str) -> ApiResult<()> {
    backend::query("UPDATE comments SET body = ?, version = ?, updated_at = ? WHERE id = ?")
        .bind(body)
        .bind(version)
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn soft_delete(c: &mut Conn, id: &str) -> ApiResult<()> {
    backend::query("UPDATE comments SET deleted_at = ? WHERE id = ?")
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}
