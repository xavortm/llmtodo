//! The order of a parent's sub-issues: the steps and the order edges.

use sqlx::SqliteConnection;

use crate::db::{new_id, now};
use crate::error::ApiResult;

/// One direct sub-issue of a parent.
#[derive(sqlx::FromRow, Clone)]
pub struct Child {
    pub id: String,
    pub key: String,
    pub order_step: Option<i64>,
    pub deleted: bool,
}

/// Every direct sub-issue of `parent_id`, trashed ones too, by step and
/// then by key number.
pub async fn children(c: &mut SqliteConnection, parent_id: &str) -> ApiResult<Vec<Child>> {
    Ok(sqlx::query_as::<_, Child>(
        "SELECT id, key, order_step, deleted_at IS NOT NULL AS deleted FROM issues
         WHERE parent_id = ? ORDER BY order_step IS NULL, order_step, team_id, number",
    )
    .bind(parent_id)
    .fetch_all(&mut *c)
    .await?)
}

/// Clears the step of every sub-issue of `parent_id`.
pub async fn clear_steps(c: &mut SqliteConnection, parent_id: &str) -> ApiResult<()> {
    sqlx::query(
        "UPDATE issues SET order_step = NULL WHERE parent_id = ? AND order_step IS NOT NULL",
    )
    .bind(parent_id)
    .execute(&mut *c)
    .await?;
    Ok(())
}

pub async fn set_step(c: &mut SqliteConnection, id: &str, step: Option<i64>) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET order_step = ? WHERE id = ?")
        .bind(step)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

/// Deletes the order edges that start at a sub-issue of `parent_id`. Order
/// edges link only siblings, so this is every order edge of the parent.
pub async fn delete_edges(c: &mut SqliteConnection, parent_id: &str) -> ApiResult<()> {
    sqlx::query(
        "DELETE FROM relations WHERE source = 'order' AND type = 'blocks'
         AND from_id IN (SELECT id FROM issues WHERE parent_id = ?)",
    )
    .bind(parent_id)
    .execute(&mut *c)
    .await?;
    Ok(())
}

/// Deletes the order edges that start or end at one issue.
pub async fn delete_edges_of(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query(
        "DELETE FROM relations WHERE source = 'order' AND type = 'blocks' AND (from_id = ? OR to_id = ?)",
    )
    .bind(id)
    .bind(id)
    .execute(&mut *c)
    .await?;
    Ok(())
}

/// Writes one order edge: `from` blocks `to`. A manual edge between the same
/// two issues in the same direction already does the job, so the insert
/// skips it.
pub async fn insert_edge(
    c: &mut SqliteConnection,
    from: &str,
    to: &str,
    actor_id: &str,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO relations (id, type, from_id, to_id, created_by, created_at, source)
         VALUES (?, 'blocks', ?, ?, ?, ?, 'order')",
    )
    .bind(new_id())
    .bind(from)
    .bind(to)
    .bind(actor_id)
    .bind(now())
    .execute(&mut *c)
    .await?;
    Ok(())
}
