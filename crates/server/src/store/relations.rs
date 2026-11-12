//! Relations between issues.

use std::collections::HashMap;

use llmtodo_api_types::{IssueLink, Relation, RelationType};
use sqlx::SqliteConnection;

use crate::db::{now, parse_ts, parse_uuid};
use crate::error::ApiResult;
use crate::store::{issues, teams};

/// The relations of one issue, with the statuses given.
pub async fn relations(
    conn: &mut SqliteConnection,
    issue_id: &str,
    statuses: &HashMap<String, teams::StatusRow>,
) -> ApiResult<Vec<Relation>> {
    let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, type, from_id, to_id, created_at FROM relations WHERE from_id = ? OR to_id = ? ORDER BY created_at",
    )
    .bind(issue_id)
    .bind(issue_id)
    .fetch_all(&mut *conn)
    .await?;
    let others: Vec<String> = rows
        .iter()
        .map(|(_, _, f, t, _)| if f == issue_id { t.clone() } else { f.clone() })
        .collect();
    // Short forms only: the full issue loader would also read claims,
    // blockers and costs that a relation never shows.
    let links: HashMap<String, IssueLink> = issues::link_map_with(conn, &others, statuses).await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, kind, from, to, created)| {
            let outgoing = from == issue_id;
            let other = if outgoing { to } else { from };
            let kind: RelationType = kind.parse().ok()?;
            let direction = match (kind, outgoing) {
                (RelationType::Blocks, true) => "blocks",
                (RelationType::Blocks, false) => "blocked-by",
                (RelationType::Related, _) => "related",
                (RelationType::Duplicate, true) => "duplicate",
                (RelationType::Duplicate, false) => "duplicated-by",
            };
            Some(Relation {
                id: parse_uuid(&id),
                direction: direction.into(),
                kind,
                issue: links.get(&other)?.clone(),
                created_at: parse_ts(&created),
            })
        })
        .collect())
}

pub async fn first_duplicate_of(c: &mut SqliteConnection, id: &str) -> ApiResult<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT 1 FROM relations WHERE type = 'duplicate' AND from_id = ?")
            .bind(id)
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn related_exists(
    c: &mut SqliteConnection,
    from: &str,
    to: &str,
) -> ApiResult<Option<i64>> {
    Ok(sqlx::query_scalar("SELECT 1 FROM relations WHERE type = 'related' AND ((from_id = ? AND to_id = ?) OR (from_id = ? AND to_id = ?))")
        .bind(from)
        .bind(to)
        .bind(to)
        .bind(from)
        .fetch_optional(&mut *c)
        .await?)
}

pub async fn exists(
    c: &mut SqliteConnection,
    kind: &str,
    from: &str,
    to: &str,
) -> ApiResult<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT 1 FROM relations WHERE type = ? AND from_id = ? AND to_id = ?")
            .bind(kind)
            .bind(from)
            .bind(to)
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn insert(
    c: &mut SqliteConnection,
    id: &str,
    kind: &str,
    from: &str,
    to: &str,
    actor_id: String,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO relations (id, type, from_id, to_id, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(id)
        .bind(kind)
        .bind(from)
        .bind(to)
        .bind(actor_id)
        .bind(now())
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn related_id(
    c: &mut SqliteConnection,
    from: &str,
    to: &str,
) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar("SELECT id FROM relations WHERE type = 'related' AND ((from_id = ? AND to_id = ?) OR (from_id = ? AND to_id = ?))")
        .bind(from)
        .bind(to)
        .bind(to)
        .bind(from)
        .fetch_optional(&mut *c)
        .await?)
}

pub async fn id_of(
    c: &mut SqliteConnection,
    kind: &str,
    from: &str,
    to: &str,
) -> ApiResult<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM relations WHERE type = ? AND from_id = ? AND to_id = ?")
            .bind(kind)
            .bind(from)
            .bind(to)
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn delete(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM relations WHERE id = ?")
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}
