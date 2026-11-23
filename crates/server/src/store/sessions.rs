use std::collections::HashMap;

use crate::store::backend::Conn;
use llmtodo_api_types::{Actor, SessionNote, WorkSession};

use crate::db::{new_id, now, parse_ts, parse_uuid};
use crate::error::ApiResult;
use crate::state::Ctx;
use crate::store::backend;

/// Adds or updates the work-session record of one agent session on each
/// issue in `issue_ids`, in one statement (LLM-469). Keeps the first `HEAD`
/// and moves the latest one. `issue_ids` must hold no issue twice.
pub fn touch_query(ctx: &Ctx, session: &str, issue_ids: &[String]) -> backend::Query {
    let c = &ctx.client;
    let t = now();
    let mut b = backend::QueryBuilder::new(
        "INSERT INTO work_sessions (id, session_id, issue_id, actor_id, harness_version, transcript_path, herdr_pane, host, worktree, branch, first_head, last_head, started_at, last_seen_at)
         VALUES ",
    );
    for (i, issue_id) in issue_ids.iter().enumerate() {
        if i > 0 {
            b.push(", ");
        }
        b.push("(")
            .push_bind(new_id())
            .push(", ")
            .push_bind(session)
            .push(", ")
            .push_bind(issue_id)
            .push(", ")
            .push_bind(ctx.actor_id())
            .push(", ")
            .push_bind(&c.harness_version)
            .push(", ")
            .push_bind(&c.transcript_path)
            .push(", ")
            .push_bind(&c.herdr_pane)
            .push(", ")
            .push_bind(&c.host)
            .push(", ")
            .push_bind(&c.worktree)
            .push(", ")
            .push_bind(&c.branch)
            .push(", ")
            .push_bind(&c.head)
            .push(", ")
            .push_bind(&c.head)
            .push(", ")
            .push_bind(&t)
            .push(", ")
            .push_bind(&t)
            .push(")");
    }
    b.push(
        " ON CONFLICT (session_id, issue_id) DO UPDATE SET
            actor_id = excluded.actor_id,
            harness_version = coalesce(excluded.harness_version, work_sessions.harness_version),
            transcript_path = coalesce(excluded.transcript_path, work_sessions.transcript_path),
            herdr_pane = coalesce(excluded.herdr_pane, work_sessions.herdr_pane),
            host = coalesce(excluded.host, work_sessions.host),
            worktree = coalesce(excluded.worktree, work_sessions.worktree),
            branch = coalesce(excluded.branch, work_sessions.branch),
            first_head = coalesce(work_sessions.first_head, excluded.first_head),
            last_head = coalesce(excluded.last_head, work_sessions.last_head),
            last_seen_at = excluded.last_seen_at",
    );
    b.build()
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

async fn notes_for(conn: &mut Conn, session_ids: &[String]) -> ApiResult<Vec<NoteRow>> {
    Ok(backend::query_as::<NoteRow>(
        "SELECT n.id, n.session_id, i.key AS issue_key, n.body, n.created_at FROM session_notes n LEFT JOIN issues i ON i.id = n.issue_id
         WHERE n.session_id IN (SELECT value FROM json_each(?)) ORDER BY n.created_at",
    )
    .bind(serde_json::to_string(session_ids)?)
    .fetch_all(&mut *conn)
    .await?)
}

async fn build(
    conn: &mut Conn,
    rows: Vec<Row>,
    actors: &HashMap<String, Actor>,
) -> ApiResult<Vec<WorkSession>> {
    let sids: Vec<String> = rows.iter().map(|r| r.session_id.clone()).collect();
    let notes = notes_for(conn, &sids).await?;
    let ws_ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let usage = crate::store::usage::by_work_session(conn, &ws_ids).await?;
    let prices = crate::store::usage::prices(conn).await?;
    Ok(assemble(rows, &notes, usage, &prices, actors))
}

type UsageBySession = std::collections::BTreeMap<
    String,
    std::collections::BTreeMap<String, llmtodo_api_types::TokenUsage>,
>;

/// The work sessions from their rows, notes and usage.
fn assemble(
    rows: Vec<Row>,
    notes: &[NoteRow],
    mut usage: UsageBySession,
    prices: &Prices,
    actors: &HashMap<String, Actor>,
) -> Vec<WorkSession> {
    rows.into_iter()
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
                crate::store::usage::summary(usage.remove(&r.id).unwrap_or_default(), prices);
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
        .collect()
}

/// The work sessions on one issue, with the model prices their usage was
/// priced with, so the caller need not read the prices again.
pub async fn for_issue(
    conn: &mut Conn,
    issue_id: &str,
    actors: &HashMap<String, Actor>,
) -> ApiResult<(Vec<WorkSession>, Prices)> {
    let rows = || {
        backend::query_as::<Row>(crate::sql!("SELECT {COLS} FROM work_sessions w JOIN issues i ON i.id = w.issue_id WHERE w.issue_id = ? ORDER BY w.started_at"))
            .bind(issue_id)
    };
    // On Postgres, outside a transaction, the four parts read by the issue
    // at once on their own connections, so they cost one round trip, not
    // four (LLM-469).
    let (rows, notes, usage, prices) = match conn.read_pool() {
        Some(pool) => {
            let c = || async { pool.acquire().await.map_err(crate::error::ApiError::from) };
            tokio::try_join!(
                async { Ok::<_, crate::error::ApiError>(rows().fetch_all(&mut c().await?).await?) },
                async { notes_for_issue(&mut c().await?, issue_id).await },
                async { crate::store::usage::by_issue_sessions(&mut c().await?, issue_id).await },
                async { crate::store::usage::prices(&mut c().await?).await },
            )?
        }
        None => (
            rows().fetch_all(&mut *conn).await?,
            notes_for_issue(conn, issue_id).await?,
            crate::store::usage::by_issue_sessions(conn, issue_id).await?,
            crate::store::usage::prices(conn).await?,
        ),
    };
    let sessions = assemble(rows, &notes, usage, &prices, actors);
    Ok((sessions, prices))
}

type Prices = std::collections::BTreeMap<String, llmtodo_api_types::ModelPrice>;

/// The notes of every session that worked on the issue, as [`notes_for`]
/// gives them for those sessions.
async fn notes_for_issue(conn: &mut Conn, issue_id: &str) -> ApiResult<Vec<NoteRow>> {
    Ok(backend::query_as::<NoteRow>(
        "SELECT n.id, n.session_id, i.key AS issue_key, n.body, n.created_at FROM session_notes n LEFT JOIN issues i ON i.id = n.issue_id
         WHERE n.session_id IN (SELECT session_id FROM work_sessions WHERE issue_id = ?) ORDER BY n.created_at",
    )
    .bind(issue_id)
    .fetch_all(&mut *conn)
    .await?)
}

pub async fn for_session(
    conn: &mut Conn,
    session_id: &str,
    actors: &HashMap<String, Actor>,
) -> ApiResult<(Vec<WorkSession>, Vec<SessionNote>)> {
    let rows = backend::query_as::<Row>(crate::sql!("SELECT {COLS} FROM work_sessions w JOIN issues i ON i.id = w.issue_id WHERE w.session_id = ? ORDER BY w.started_at"))
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

pub async fn note_by_id(conn: &mut Conn, id: &str) -> ApiResult<SessionNote> {
    let r = backend::query_as::<NoteRow>("SELECT n.id, n.session_id, i.key AS issue_key, n.body, n.created_at FROM session_notes n LEFT JOIN issues i ON i.id = n.issue_id WHERE n.id = ?")
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(note(&r))
}

pub async fn add_note(
    c: &mut Conn,
    id: &str,
    session_id: &str,
    issue_id: Option<&str>,
    body: &str,
    actor_id: String,
) -> ApiResult<()> {
    backend::query("INSERT INTO session_notes (id, session_id, issue_id, body, actor_id, created_at) VALUES (?, ?, ?, ?, ?, ?)")
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
