use std::collections::HashMap;

use llmtodo_api_types::{Actor, SessionNote, WorkSession};
use sqlx::SqliteConnection;

use crate::db::{new_id, now, parse_ts, parse_uuid};
use crate::error::ApiResult;
use crate::state::Ctx;

/// Adds or updates the work-session record of one agent session on one
/// issue. Keeps the first `HEAD` and moves the latest one.
pub async fn touch(
    conn: &mut SqliteConnection,
    ctx: &Ctx,
    session: &str,
    issue_id: &str,
) -> ApiResult<()> {
    let c = &ctx.client;
    let t = now();
    sqlx::query(
        "INSERT INTO work_sessions (id, session_id, issue_id, actor_id, harness_version, transcript_path, herdr_pane, host, worktree, branch, first_head, last_head, started_at, last_seen_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (session_id, issue_id) DO UPDATE SET
            actor_id = excluded.actor_id,
            harness_version = coalesce(excluded.harness_version, harness_version),
            transcript_path = coalesce(excluded.transcript_path, transcript_path),
            herdr_pane = coalesce(excluded.herdr_pane, herdr_pane),
            host = coalesce(excluded.host, host),
            worktree = coalesce(excluded.worktree, worktree),
            branch = coalesce(excluded.branch, branch),
            first_head = coalesce(first_head, excluded.first_head),
            last_head = coalesce(excluded.last_head, last_head),
            last_seen_at = excluded.last_seen_at",
    )
    .bind(new_id())
    .bind(session)
    .bind(issue_id)
    .bind(ctx.actor_id())
    .bind(&c.harness_version)
    .bind(&c.transcript_path)
    .bind(&c.herdr_pane)
    .bind(&c.host)
    .bind(&c.worktree)
    .bind(&c.branch)
    .bind(&c.head)
    .bind(&c.head)
    .bind(&t)
    .bind(&t)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    session_id: String,
    issue_key: String,
    actor_id: String,
    harness_version: Option<String>,
    transcript_path: Option<String>,
    herdr_pane: Option<String>,
    host: Option<String>,
    worktree: Option<String>,
    branch: Option<String>,
    first_head: Option<String>,
    last_head: Option<String>,
    started_at: String,
    last_seen_at: String,
    active_seconds: i64,
}

const COLS: &str = "w.id, w.session_id, i.key AS issue_key, w.actor_id, w.harness_version, w.transcript_path, w.herdr_pane, w.host,
    w.worktree, w.branch, w.first_head, w.last_head, w.started_at, w.last_seen_at, w.active_seconds";

#[derive(sqlx::FromRow)]
struct NoteRow {
    id: String,
    session_id: String,
    issue_key: Option<String>,
    body: String,
    created_at: String,
}

fn note(r: &NoteRow) -> SessionNote {
    SessionNote {
        id: parse_uuid(&r.id),
        session_id: r.session_id.clone(),
        issue: r.issue_key.as_deref().and_then(|k| k.parse().ok()),
        body: r.body.clone(),
        created_at: parse_ts(&r.created_at),
    }
}

async fn notes_for(conn: &mut SqliteConnection, session_ids: &[String]) -> ApiResult<Vec<NoteRow>> {
    Ok(sqlx::query_as::<_, NoteRow>(
        "SELECT n.id, n.session_id, i.key AS issue_key, n.body, n.created_at FROM session_notes n LEFT JOIN issues i ON i.id = n.issue_id
         WHERE n.session_id IN (SELECT value FROM json_each(?)) ORDER BY n.created_at",
    )
    .bind(serde_json::to_string(session_ids)?)
    .fetch_all(&mut *conn)
    .await?)
}

async fn build(
    conn: &mut SqliteConnection,
    rows: Vec<Row>,
    actors: &HashMap<String, Actor>,
) -> ApiResult<Vec<WorkSession>> {
    let sids: Vec<String> = rows.iter().map(|r| r.session_id.clone()).collect();
    let notes = notes_for(conn, &sids).await?;
    let ws_ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let mut usage = crate::store::usage::by_work_session(conn, &ws_ids).await?;
    let prices = crate::store::usage::prices(conn).await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let actor = actors.get(&r.actor_id)?.clone();
            let my_notes = notes
                .iter()
                .filter(|n| {
                    n.session_id == r.session_id
                        && n.issue_key.as_deref().is_none_or(|k| k == r.issue_key)
                })
                .map(note)
                .collect();
            let used =
                crate::store::usage::summary(usage.remove(&r.id).unwrap_or_default(), &prices);
            Some(WorkSession {
                id: parse_uuid(&r.id),
                session_id: r.session_id,
                issue: r.issue_key.parse().ok()?,
                actor,
                harness_version: r.harness_version,
                transcript_path: r.transcript_path,
                herdr_pane: r.herdr_pane,
                host: r.host,
                worktree: r.worktree,
                branch: r.branch,
                first_head: r.first_head,
                last_head: r.last_head,
                commits: Vec::new(),
                started_at: parse_ts(&r.started_at),
                last_seen_at: parse_ts(&r.last_seen_at),
                notes: my_notes,
                usage: used,
                active_seconds: (r.active_seconds > 0).then_some(r.active_seconds as u64),
            })
        })
        .collect())
}

pub async fn for_issue(
    conn: &mut SqliteConnection,
    issue_id: &str,
    actors: &HashMap<String, Actor>,
) -> ApiResult<Vec<WorkSession>> {
    let rows = sqlx::query_as::<_, Row>(crate::sql!("SELECT {COLS} FROM work_sessions w JOIN issues i ON i.id = w.issue_id WHERE w.issue_id = ? ORDER BY w.started_at"))
        .bind(issue_id)
        .fetch_all(&mut *conn)
        .await?;
    build(conn, rows, actors).await
}

pub async fn for_session(
    conn: &mut SqliteConnection,
    session_id: &str,
    actors: &HashMap<String, Actor>,
) -> ApiResult<(Vec<WorkSession>, Vec<SessionNote>)> {
    let rows = sqlx::query_as::<_, Row>(crate::sql!("SELECT {COLS} FROM work_sessions w JOIN issues i ON i.id = w.issue_id WHERE w.session_id = ? ORDER BY w.started_at"))
        .bind(session_id)
        .fetch_all(&mut *conn)
        .await?;
    let issues = build(conn, rows, actors).await?;
    let notes = notes_for(conn, &[session_id.to_string()])
        .await?
        .iter()
        .map(note)
        .collect();
    Ok((issues, notes))
}

pub async fn note_by_id(conn: &mut SqliteConnection, id: &str) -> ApiResult<SessionNote> {
    let r = sqlx::query_as::<_, NoteRow>("SELECT n.id, n.session_id, i.key AS issue_key, n.body, n.created_at FROM session_notes n LEFT JOIN issues i ON i.id = n.issue_id WHERE n.id = ?")
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(note(&r))
}

pub async fn add_note(
    c: &mut SqliteConnection,
    id: &str,
    session_id: &str,
    issue_id: Option<&str>,
    body: &str,
    actor_id: String,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO session_notes (id, session_id, issue_id, body, actor_id, created_at) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(id)
        .bind(session_id)
        .bind(issue_id)
        .bind(body)
        .bind(actor_id)
        .bind(now())
        .execute(&mut *c)
        .await?;
    Ok(())
}
