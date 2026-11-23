//! The outbox of offline changes, and the reads that replay compares
//! (LLM-471).
//!
//! The outbox lives only in the offline copy, `mirror.db`. It is not part
//! of the shared schema, so it has no migration in either folder: the copy
//! creates the table when it opens, and a refresh of the copy carries its
//! rows over.

use serde_json::{Value, json};

use crate::error::ApiResult;
use crate::store::backend::{self, Conn};

const CREATE: &str = "CREATE TABLE IF NOT EXISTS outbox (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    method     TEXT NOT NULL,
    path       TEXT NOT NULL,
    body       TEXT,
    context    TEXT,
    issue_id   TEXT,
    seen       TEXT,
    after      TEXT,
    created_at TEXT NOT NULL,
    state      TEXT NOT NULL DEFAULT 'pending',
    error      TEXT
)";

/// One change made offline.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Row {
    pub id: i64,
    pub method: String,
    pub path: String,
    /// The request JSON, with provisional keys already turned into IDs.
    pub body: Option<String>,
    /// The caller's context header, base64url JSON.
    pub context: Option<String>,
    pub issue_id: Option<String>,
    /// The issue as the change found it: see [`snapshot`].
    pub seen: Option<String>,
    /// What the change left: the description, and for a create the new
    /// issue's ID and provisional key.
    pub after: Option<String>,
    pub created_at: String,
    /// `pending` or `failed`.
    pub state: String,
    /// The error body of a refused replay.
    pub error: Option<String>,
}

const COLUMNS: &str =
    "id, method, path, body, context, issue_id, seen, after, created_at, state, error";

pub async fn create_table(c: &mut Conn) -> ApiResult<()> {
    backend::query(CREATE).execute(&mut *c).await?;
    Ok(())
}

/// Everything a new row holds.
pub struct NewRow<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub body: Option<&'a str>,
    pub context: Option<&'a str>,
    pub issue_id: Option<&'a str>,
    pub seen: Option<String>,
    pub after: Option<String>,
    pub created_at: &'a str,
}

pub async fn insert(c: &mut Conn, r: NewRow<'_>) -> ApiResult<()> {
    backend::query(
        "INSERT INTO outbox (method, path, body, context, issue_id, seen, after, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(r.method)
    .bind(r.path)
    .bind(r.body)
    .bind(r.context)
    .bind(r.issue_id)
    .bind(r.seen)
    .bind(r.after)
    .bind(r.created_at)
    .execute(&mut *c)
    .await?;
    Ok(())
}

pub async fn all(c: &mut Conn) -> ApiResult<Vec<Row>> {
    Ok(
        backend::query_as::<Row>(crate::sql!("SELECT {COLUMNS} FROM outbox ORDER BY id"))
            .fetch_all(&mut *c)
            .await?,
    )
}

/// The oldest pending row after `after`.
pub async fn next_pending(c: &mut Conn, after: i64) -> ApiResult<Option<Row>> {
    Ok(backend::query_as::<Row>(crate::sql!(
        "SELECT {COLUMNS} FROM outbox WHERE state = 'pending' AND id > ? ORDER BY id LIMIT 1"
    ))
    .bind(after)
    .fetch_optional(&mut *c)
    .await?)
}

/// Pending and failed rows.
pub async fn counts(c: &mut Conn) -> ApiResult<(u64, u64)> {
    let (pending, failed): (i64, i64) = backend::query_as(
        "SELECT coalesce(sum(CASE WHEN state = 'pending' THEN 1 ELSE 0 END), 0),
                coalesce(sum(CASE WHEN state = 'failed' THEN 1 ELSE 0 END), 0)
         FROM outbox",
    )
    .fetch_one(&mut *c)
    .await?;
    Ok((pending as u64, failed as u64))
}

pub async fn delete(c: &mut Conn, id: i64) -> ApiResult<bool> {
    let done = backend::query("DELETE FROM outbox WHERE id = ?")
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn mark_failed(c: &mut Conn, id: i64, error: &str) -> ApiResult<()> {
    backend::query("UPDATE outbox SET state = 'failed', error = ? WHERE id = ?")
        .bind(error)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

/// Copies every row into another copy's outbox, keeping IDs.
pub async fn carry(from: &mut Conn, to: &mut Conn) -> ApiResult<()> {
    for r in all(from).await? {
        backend::query(crate::sql!(
            "INSERT INTO outbox ({COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(r.id)
        .bind(r.method)
        .bind(r.path)
        .bind(r.body)
        .bind(r.context)
        .bind(r.issue_id)
        .bind(r.seen)
        .bind(r.after)
        .bind(r.created_at)
        .bind(r.state)
        .bind(r.error)
        .execute(&mut *to)
        .await?;
    }
    Ok(())
}

/// The fields of an issue that replay compares, as JSON. Each key names a
/// group of `UpdateIssue` fields: see `offline::FIELDS`. IDs, not names,
/// so the offline copy and the shared database agree.
pub async fn snapshot(c: &mut Conn, issue_id: &str) -> ApiResult<Option<Value>> {
    #[allow(clippy::type_complexity)]
    let row: Option<(
        String,
        String,
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        String,
        Option<String>,
        String,
        Option<i64>,
    )> = backend::query_as(
        "SELECT title, status_id, priority, assignee_id, project_id, milestone_id, parent_id,
                order_step, team_id, waiting_on, wanted_executor, estimate_minutes
         FROM issues WHERE id = ?",
    )
    .bind(issue_id)
    .fetch_optional(&mut *c)
    .await?;
    let Some(r) = row else {
        return Ok(None);
    };
    let (cost, branch, worktree, pr, description, version): (
        Option<f64>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        i64,
    ) = backend::query_as(
        "SELECT estimate_cost_usd, git_branch, git_worktree, git_pr, description, description_version
         FROM issues WHERE id = ?",
    )
    .bind(issue_id)
    .fetch_one(&mut *c)
    .await?;
    let labels: Vec<String> = backend::query_scalar(
        "SELECT label_id FROM issue_labels WHERE issue_id = ? ORDER BY label_id",
    )
    .bind(issue_id)
    .fetch_all(&mut *c)
    .await?;
    Ok(Some(json!({
        "title": r.0,
        "status": r.1,
        "priority": r.2,
        "assignee": r.3,
        "project": r.4,
        "milestone": r.5,
        "parent": r.6,
        "order_step": r.7,
        "team": r.8,
        "waiting_on": r.9,
        "wanted_executor": r.10,
        "estimates": [r.11, cost],
        "git": [branch, worktree, pr],
        "labels": labels,
        "description": description,
        "description_version": version,
    })))
}

/// When the shared database last changed any of `fields` of an issue, from
/// its activity, as stored text. `None` when it never did.
pub async fn last_change(
    c: &mut Conn,
    issue_id: &str,
    fields: &[&str],
) -> ApiResult<Option<String>> {
    let mut q =
        backend::QueryBuilder::new("SELECT max(created_at) FROM activity WHERE issue_id = ");
    q.push_bind(issue_id);
    q.push(" AND field IN (");
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            q.push(", ");
        }
        q.push_bind(*f);
    }
    q.push(")");
    Ok(q.build_query_scalar::<Option<String>>()
        .fetch_one(&mut *c)
        .await?)
}

/// The highest provisional number given in a team, from the keys this copy
/// holds, so a new one never repeats one that replay kept as an alias.
pub async fn last_provisional(c: &mut Conn, team_key: &str) -> ApiResult<u32> {
    let keys: Vec<String> = backend::query_scalar("SELECT key FROM issue_keys WHERE key LIKE ?")
        .bind(format!("{team_key}-~%"))
        .fetch_all(&mut *c)
        .await?;
    Ok(keys
        .iter()
        .filter_map(|k| k.parse::<llmtodo_core::IssueKey>().ok())
        .filter(|k| k.provisional)
        .map(|k| k.number)
        .max()
        .unwrap_or(0))
}

/// The newest activity seq a copy holds.
pub async fn max_seq(c: &mut Conn) -> ApiResult<i64> {
    Ok(
        backend::query_scalar("SELECT coalesce(max(seq), 0) FROM activity")
            .fetch_one(&mut *c)
            .await?,
    )
}

/// The activity after `seq`: each event's seq and issue, or `None` for one
/// that names no issue, such as a label change.
pub async fn changed_after(
    c: &mut Conn,
    seq: i64,
    limit: i64,
) -> ApiResult<Vec<(i64, Option<String>)>> {
    Ok(
        backend::query_as("SELECT seq, issue_id FROM activity WHERE seq > ? ORDER BY seq LIMIT ?")
            .bind(seq)
            .bind(limit)
            .fetch_all(&mut *c)
            .await?,
    )
}
