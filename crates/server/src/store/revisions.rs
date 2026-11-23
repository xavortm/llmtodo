//! Saved versions of issue, comment and document text.
//!
//! A revision row holds either the full text or a delta against the next
//! newer row of the same entity (LLM-346). The newest row is always full, so
//! adding a version rewrites the previous newest row as a delta against the
//! new text. Every `FULL_EVERY`th version stays full, so a read applies at
//! most that many deltas. Callers see only full text.

use std::collections::BTreeMap;

use crate::store::backend::{Conn, Pool};
use llmtodo_api_types::Revision;

use crate::db::{new_id, now, parse_ts};
use crate::delta;
use crate::error::{ApiError, ApiResult};
use crate::store::actors;
use crate::store::backend;

/// Every version divisible by this keeps its full text.
const FULL_EVERY: i64 = 32;

/// The settings row that marks the one-time compaction of old full copies.
const COMPACT_KEY: &str = "revisions_compact";

/// A stored row: version, body, and the base version when body is a delta.
type Stored = (i64, String, Option<i64>);

/// Rebuilds the full text of every row. A delta's base is newer than the
/// delta, so the rows are read newest first.
fn rebuild(mut rows: Vec<Stored>) -> ApiResult<BTreeMap<i64, String>> {
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    let mut texts: BTreeMap<i64, String> = BTreeMap::new();
    for (version, body, base) in rows {
        let text = match base {
            None => body,
            Some(b) => {
                let base_text = texts.get(&b).ok_or_else(|| {
                    ApiError::internal(format!(
                        "revision {version} is stored against revision {b}, which is missing"
                    ))
                })?;
                delta::apply(base_text, &body).map_err(|e| {
                    ApiError::internal(format!("revision {version} cannot be read: {e}"))
                })?
            }
        };
        texts.insert(version, text);
    }
    Ok(texts)
}

/// The insert of version 1 of a new record. There is no older row to lean
/// on, so it needs no read first (LLM-469).
pub fn first_revision_query(
    entity_type: &str,
    entity_id: &str,
    body: &str,
    actor_id: &str,
    session: Option<&str>,
) -> backend::Query {
    insert_revision_query(entity_type, entity_id, 1, body, actor_id, session)
}

fn insert_revision_query(
    entity_type: &str,
    entity_id: &str,
    version: i64,
    body: &str,
    actor_id: &str,
    session: Option<&str>,
) -> backend::Query {
    backend::query("INSERT INTO revisions (id, entity_type, entity_id, version, body, actor_id, session_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(new_id())
        .bind(entity_type)
        .bind(entity_id)
        .bind(version)
        .bind(body)
        .bind(actor_id)
        .bind(session)
        .bind(now())
}

pub async fn add_revision(
    conn: &mut Conn,
    entity_type: &str,
    entity_id: &str,
    version: i64,
    body: &str,
    actor_id: &str,
    session: Option<&str>,
) -> ApiResult<()> {
    let newest: Option<Stored> = backend::query_as(
        "SELECT version, body, base_version FROM revisions WHERE entity_type = ? AND entity_id = ? ORDER BY version DESC LIMIT 1",
    )
    .bind(entity_type)
    .bind(entity_id)
    .fetch_optional(&mut *conn)
    .await?;
    insert_revision_query(entity_type, entity_id, version, body, actor_id, session)
        .execute(&mut *conn)
        .await?;
    // The row that was newest now has a newer row to lean on.
    if let Some((prev, prev_body, None)) = newest
        && prev < version
        && prev % FULL_EVERY != 0
        && let Some(d) = delta::encode(body, &prev_body)
    {
        set_delta(conn, entity_type, entity_id, prev, &d, version).await?;
    }
    Ok(())
}

async fn set_delta(
    c: &mut Conn,
    entity_type: &str,
    entity_id: &str,
    version: i64,
    delta: &str,
    base: i64,
) -> ApiResult<()> {
    backend::query("UPDATE revisions SET body = ?, base_version = ? WHERE entity_type = ? AND entity_id = ? AND version = ?")
        .bind(delta)
        .bind(base)
        .bind(entity_type)
        .bind(entity_id)
        .bind(version)
        .execute(&mut *c)
        .await?;
    Ok(())
}

async fn stored_rows(c: &mut Conn, entity_type: &str, entity_id: &str) -> ApiResult<Vec<Stored>> {
    Ok(backend::query_as(
        "SELECT version, body, base_version FROM revisions WHERE entity_type = ? AND entity_id = ?",
    )
    .bind(entity_type)
    .bind(entity_id)
    .fetch_all(&mut *c)
    .await?)
}

/// Every revision of an entity, oldest first. The text comes only with
/// `bodies`.
pub async fn revisions(
    conn: &mut Conn,
    entity_type: &str,
    entity_id: &str,
    bodies: bool,
) -> ApiResult<Vec<Revision>> {
    let actors = actors::all(conn).await?;
    let mut texts = if bodies {
        rebuild(stored_rows(conn, entity_type, entity_id).await?)?
    } else {
        BTreeMap::new()
    };
    type Row = (i64, Option<String>, Option<String>, String);
    let rows: Vec<Row> = backend::query_as(
        "SELECT version, actor_id, session_id, created_at FROM revisions WHERE entity_type = ? AND entity_id = ? ORDER BY version",
    )
    .bind(entity_type)
    .bind(entity_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(version, actor, session_id, created)| Revision {
            version,
            body: texts.remove(&version),
            actor: actor.and_then(|a| actors.get(&a).cloned()),
            session_id,
            created_at: parse_ts(&created),
        })
        .collect())
}

pub async fn revision_body(
    conn: &mut Conn,
    entity_type: &str,
    entity_id: &str,
    version: i64,
) -> ApiResult<String> {
    // The chain from a version runs up to the nearest full row above it.
    let rows: Vec<Stored> = backend::query_as(
        "SELECT version, body, base_version FROM revisions
         WHERE entity_type = ?1 AND entity_id = ?2 AND version >= ?3
         AND version <= coalesce((SELECT min(version) FROM revisions
             WHERE entity_type = ?1 AND entity_id = ?2 AND version >= ?3 AND base_version IS NULL), version)",
    )
    .bind(entity_type)
    .bind(entity_id)
    .bind(version)
    .fetch_all(&mut *conn)
    .await?;
    if !rows.iter().any(|r| r.0 == version) {
        return Err(ApiError::not_found("revision", &version.to_string()));
    }
    // A row added out of order can sit inside a chain. Then read them all.
    let has = |v: i64| rows.iter().any(|r| r.0 == v);
    let rows = if rows.iter().all(|r| r.2.is_none_or(has)) {
        rows
    } else {
        backend::query_as(
            "SELECT version, body, base_version FROM revisions WHERE entity_type = ? AND entity_id = ? AND version >= ?",
        )
        .bind(entity_type)
        .bind(entity_id)
        .bind(version)
        .fetch_all(&mut *conn)
        .await?
    };
    rebuild(rows)?
        .remove(&version)
        .ok_or_else(|| ApiError::not_found("revision", &version.to_string()))
}

pub async fn of_issue_oldest_first(
    c: &mut Conn,
    issue_id: &str,
) -> ApiResult<Vec<(i64, String, Option<String>)>> {
    let actors: Vec<(i64, Option<String>)> = backend::query_as(
        "SELECT version, actor_id FROM revisions
         WHERE entity_type = 'issue' AND entity_id = ? ORDER BY version",
    )
    .bind(issue_id)
    .fetch_all(&mut *c)
    .await?;
    let mut texts = rebuild(stored_rows(c, "issue", issue_id).await?)?;
    Ok(actors
        .into_iter()
        .map(|(version, actor)| (version, texts.remove(&version).unwrap_or_default(), actor))
        .collect())
}

pub async fn delete_of_issue(c: &mut Conn, id: &str) -> ApiResult<()> {
    backend::query("DELETE FROM revisions WHERE entity_type = 'issue' AND entity_id = ?")
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn delete_of_comments(c: &mut Conn, issue_id: &str) -> ApiResult<()> {
    backend::query("DELETE FROM revisions WHERE entity_type = 'comment' AND entity_id IN (SELECT id FROM comments WHERE issue_id = ?)")
        .bind(issue_id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

/// The rows a trim deletes: older than the cut, and not the newest row of
/// their entity.
const OLD: &str = "r.created_at < ?1 AND r.version < (SELECT max(version) FROM revisions r2
    WHERE r2.entity_type = r.entity_type AND r2.entity_id = r.entity_id)";

pub async fn delete_old(c: &mut Conn, before: String) -> ApiResult<()> {
    // A kept delta whose base goes too gets its full text first. Bases are
    // newer than their deltas, so this happens only when created_at runs
    // against the version order.
    let orphans: Vec<(String, String, i64)> = backend::query_as(crate::sql!(
        "SELECT k.entity_type, k.entity_id, k.version FROM revisions k
         JOIN revisions r ON r.entity_type = k.entity_type AND r.entity_id = k.entity_id AND r.version = k.base_version
         WHERE {OLD} AND NOT (k.created_at < ?1 AND k.version < (SELECT max(version) FROM revisions r3
             WHERE r3.entity_type = k.entity_type AND r3.entity_id = k.entity_id))"
    ))
    .bind(&before)
    .fetch_all(&mut *c)
    .await?;
    for (entity_type, entity_id, version) in orphans {
        let text = revision_body(c, &entity_type, &entity_id, version).await?;
        backend::query("UPDATE revisions SET body = ?, base_version = NULL WHERE entity_type = ? AND entity_id = ? AND version = ?")
            .bind(text)
            .bind(&entity_type)
            .bind(&entity_id)
            .bind(version)
            .execute(&mut *c)
            .await?;
    }
    backend::query(crate::sql!(
        "DELETE FROM revisions WHERE id IN (SELECT r.id FROM revisions r WHERE {OLD})"
    ))
    .bind(before)
    .execute(&mut *c)
    .await?;
    Ok(())
}

pub async fn count_old(c: &mut Conn, before: String) -> ApiResult<i64> {
    Ok(
        backend::query_scalar(crate::sql!("SELECT count(*) FROM revisions r WHERE {OLD}"))
            .bind(before)
            .fetch_one(&mut *c)
            .await?,
    )
}

/// Turns the full copies written before LLM-346 into deltas, once, after the
/// migration that adds `base_version`. The text of every revision stays the
/// same. One transaction clears the pending mark too, so a crash leaves the
/// work to the next start.
pub async fn compact(pool: &Pool) -> ApiResult<()> {
    async fn pending(c: &mut Conn) -> ApiResult<bool> {
        let found: Option<String> =
            backend::query_scalar("SELECT value FROM settings WHERE key = ?")
                .bind(COMPACT_KEY)
                .fetch_optional(&mut *c)
                .await?;
        Ok(found.is_some())
    }
    // A plain read first, so a start with nothing to do takes no write lock.
    if !pending(&mut pool.acquire().await?).await? {
        return Ok(());
    }
    let mut tx = pool.begin_write().await?;
    if !pending(&mut tx).await? {
        return Ok(());
    }
    let entities: Vec<(String, String)> = backend::query_as(
        "SELECT entity_type, entity_id FROM revisions GROUP BY entity_type, entity_id HAVING count(*) > 1",
    )
    .fetch_all(&mut tx)
    .await?;
    let mut compacted = 0;
    for (entity_type, entity_id) in &entities {
        let texts = rebuild(stored_rows(&mut tx, entity_type, entity_id).await?)?;
        let rows: Vec<(&i64, &String)> = texts.iter().collect();
        for pair in rows.windows(2) {
            let ((&version, text), (&base, base_text)) = (pair[0], pair[1]);
            if version % FULL_EVERY == 0 {
                continue;
            }
            if let Some(d) = delta::encode(base_text, text) {
                set_delta(&mut tx, entity_type, entity_id, version, &d, base).await?;
                compacted += 1;
            }
        }
    }
    backend::query("DELETE FROM settings WHERE key = ?")
        .bind(COMPACT_KEY)
        .execute(&mut tx)
        .await?;
    tx.commit().await?;
    tracing::info!(revisions = compacted, "old revisions stored as deltas");
    Ok(())
}
