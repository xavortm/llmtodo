//! Loading, resolving and querying issues.

use std::collections::HashMap;

use base64::Engine;
use llmtodo_api_types::{
    ANALYZING_EXPIRY_SECS, Actor, AnalyzingSummary, ClaimSummary, Issue, IssueKey, IssueLink,
    IssueQuery, Priority, WantedExecutor,
};
use serde_json::Value;
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

use crate::db::{now, parse_ts, parse_ts_opt, parse_uuid};
use crate::error::{ApiError, ApiResult};
use crate::store::{actors, deps, filter, projects, teams};

/// Resolves a key (current or old) or an ID. Trashed issues resolve too;
/// callers that need a live issue check `deleted_at`.
pub async fn resolve_id(conn: &mut SqliteConnection, r: &str) -> ApiResult<String> {
    let r = r.trim();
    if let Ok(key) = r.parse::<IssueKey>() {
        let id: Option<String> =
            sqlx::query_scalar("SELECT issue_id FROM issue_keys WHERE key = ?")
                .bind(key.to_string())
                .fetch_optional(&mut *conn)
                .await?;
        return id.ok_or_else(|| ApiError::not_found("issue", r));
    }
    let id: Option<String> = sqlx::query_scalar("SELECT id FROM issues WHERE id = ?")
        .bind(r)
        .fetch_optional(&mut *conn)
        .await?;
    id.ok_or_else(|| ApiError::not_found("issue", r))
}

/// Like [`resolve_id`], but a trashed issue counts as not found.
pub async fn resolve_live(conn: &mut SqliteConnection, r: &str) -> ApiResult<IssueRow> {
    let id = resolve_id(conn, r).await?;
    let row = row(conn, &id).await?;
    if row.deleted_at.is_some() {
        return Err(ApiError::not_found("issue", r)
            .with_details(serde_json::json!({ "ref": r, "trashed": true })));
    }
    Ok(row)
}

#[derive(sqlx::FromRow, Clone)]
pub struct IssueRow {
    pub id: String,
    pub key: String,
    pub team_id: String,
    pub team_key: String,
    pub number: i64,
    pub title: String,
    pub description: Option<String>,
    pub description_version: i64,
    pub status_id: String,
    pub priority: i64,
    pub assignee_id: Option<String>,
    pub project_id: Option<String>,
    pub project_slug: Option<String>,
    pub milestone_id: Option<String>,
    pub milestone_name: Option<String>,
    pub parent_id: Option<String>,
    pub parent_key: Option<String>,
    pub wanted_executor: String,
    pub waiting_on: Option<String>,
    pub estimate_minutes: Option<i64>,
    pub estimate_cost_usd: Option<f64>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub deleted_at: Option<String>,
}

impl IssueRow {
    pub fn executor(&self) -> WantedExecutor {
        serde_json::from_str(&self.wanted_executor).unwrap_or_default()
    }
}

const SELECT: &str = "SELECT i.id, i.key, i.team_id, t.key AS team_key, i.number, i.title,
    CASE WHEN ? THEN i.description END AS description, i.description_version, i.status_id, i.priority, i.assignee_id,
    i.project_id, p.slug AS project_slug, i.milestone_id, m.name AS milestone_name, i.parent_id, par.key AS parent_key,
    i.wanted_executor, i.waiting_on, i.estimate_minutes, i.estimate_cost_usd, i.created_by, i.created_at, i.updated_at, i.completed_at, i.deleted_at
    FROM issues i
    JOIN teams t ON t.id = i.team_id
    LEFT JOIN projects p ON p.id = i.project_id
    LEFT JOIN milestones m ON m.id = i.milestone_id
    LEFT JOIN issues par ON par.id = i.parent_id";

pub async fn row(conn: &mut SqliteConnection, id: &str) -> ApiResult<IssueRow> {
    sqlx::query_as::<_, IssueRow>(crate::sql!("{SELECT} WHERE i.id = ?"))
        .bind(true)
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| ApiError::not_found("issue", id))
}

async fn rows(
    conn: &mut SqliteConnection,
    ids: &[String],
    with_description: bool,
) -> ApiResult<Vec<IssueRow>> {
    let json = serde_json::to_string(ids)?;
    Ok(sqlx::query_as::<_, IssueRow>(crate::sql!(
        "{SELECT} WHERE i.id IN (SELECT value FROM json_each(?))"
    ))
    .bind(with_description)
    .bind(json)
    .fetch_all(&mut *conn)
    .await?)
}

#[derive(sqlx::FromRow)]
struct ClaimRow {
    issue_id: String,
    actor_id: String,
    session_id: String,
    host: Option<String>,
    worktree: Option<String>,
    branch: Option<String>,
    herdr_pane: Option<String>,
    created_at: String,
}

pub async fn live_claims(
    conn: &mut SqliteConnection,
    ids: &[String],
    actors: &HashMap<String, Actor>,
) -> ApiResult<HashMap<String, ClaimSummary>> {
    let json = serde_json::to_string(ids)?;
    let rows = sqlx::query_as::<_, ClaimRow>(
        "SELECT issue_id, actor_id, session_id, host, worktree, branch, herdr_pane, created_at FROM claims
         WHERE ended_at IS NULL AND issue_id IN (SELECT value FROM json_each(?))",
    )
    .bind(json)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let actor = actors.get(&r.actor_id)?.clone();
            Some((
                r.issue_id,
                ClaimSummary {
                    actor,
                    session_id: r.session_id,
                    host: r.host,
                    worktree: r.worktree,
                    branch: r.branch,
                    herdr_pane: r.herdr_pane,
                    claimed_at: parse_ts(&r.created_at),
                },
            ))
        })
        .collect())
}

/// The oldest `seen_at` a live Analyzing mark may have now.
pub fn analyzing_cutoff() -> String {
    let t = jiff::Timestamp::now() - jiff::SignedDuration::from_secs(ANALYZING_EXPIRY_SECS);
    crate::db::ts(t)
}

/// Live Analyzing marks by issue ID. Expired marks are left out.
pub async fn live_analyzing(
    conn: &mut SqliteConnection,
    ids: &[String],
    actors: &HashMap<String, Actor>,
) -> ApiResult<HashMap<String, AnalyzingSummary>> {
    let json = serde_json::to_string(ids)?;
    let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT issue_id, actor_id, session_id, started_at, seen_at FROM analyzing
         WHERE seen_at >= ? AND issue_id IN (SELECT value FROM json_each(?))",
    )
    .bind(analyzing_cutoff())
    .bind(json)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(issue, actor, session_id, started, seen)| {
            let actor = actors.get(&actor)?.clone();
            let seen_at = parse_ts(&seen);
            Some((
                issue,
                AnalyzingSummary {
                    actor,
                    session_id,
                    started_at: parse_ts(&started),
                    seen_at,
                    expires_at: seen_at + jiff::SignedDuration::from_secs(ANALYZING_EXPIRY_SECS),
                },
            ))
        })
        .collect())
}

/// The statuses and actors that issue rows point at. A request that builds
/// issues in several steps loads them once and passes them down.
pub struct Lookup {
    pub statuses: HashMap<String, teams::StatusRow>,
    pub actors: HashMap<String, Actor>,
}

impl Lookup {
    pub async fn load(conn: &mut SqliteConnection) -> ApiResult<Self> {
        Ok(Self {
            statuses: teams::status_map(conn).await?,
            actors: actors::all(conn).await?,
        })
    }
}

/// Loads issues in the order of `ids`.
pub async fn load(
    conn: &mut SqliteConnection,
    ids: &[String],
    with_description: bool,
) -> ApiResult<Vec<Issue>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let lookup = Lookup::load(conn).await?;
    load_with(conn, ids, with_description, &lookup).await
}

/// Loads issues in the order of `ids`, with statuses and actors given.
pub async fn load_with(
    conn: &mut SqliteConnection,
    ids: &[String],
    with_description: bool,
    lookup: &Lookup,
) -> ApiResult<Vec<Issue>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = rows(conn, ids, with_description).await?;
    let statuses = &lookup.statuses;
    let actors = &lookup.actors;
    let claims = live_claims(conn, ids, actors).await?;
    let analyzing = live_analyzing(conn, ids, actors).await?;
    let blockers = deps::Blockers::load(conn, ids, statuses).await?;
    let mut costs = crate::store::usage::cost_by_issue(conn, ids).await?;
    let open_questions = crate::store::questions::open_counts(conn, ids).await?;
    let mut times = crate::store::usage::active_seconds_by_issue(conn, ids).await?;
    let json = serde_json::to_string(ids)?;
    let label_rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT il.issue_id, l.name, g.name FROM issue_labels il JOIN labels l ON l.id = il.label_id LEFT JOIN labels g ON g.id = l.group_id
         WHERE il.issue_id IN (SELECT value FROM json_each(?)) ORDER BY coalesce(g.name, l.name) COLLATE NOCASE, l.name COLLATE NOCASE",
    )
    .bind(json)
    .fetch_all(&mut *conn)
    .await?;
    let mut labels_by: HashMap<String, Vec<String>> = HashMap::new();
    for (issue, name, group) in label_rows {
        labels_by.entry(issue).or_default().push(match group {
            Some(g) => format!("{g}/{name}"),
            None => name,
        });
    }
    let mut by_id: HashMap<String, Issue> = HashMap::new();
    for r in rows {
        let status = statuses
            .get(&r.status_id)
            .map(super::teams::StatusRow::to_ref)
            .ok_or_else(|| ApiError::internal("issue has an unknown status"))?;
        let key: IssueKey = r
            .key
            .parse()
            .map_err(|_| ApiError::internal(format!("bad key {}", r.key)))?;
        let issue = Issue {
            id: parse_uuid(&r.id),
            key,
            team: r.team_key.clone(),
            title: r.title.clone(),
            status,
            priority: Priority::from_number(r.priority).unwrap_or(Priority::None),
            assignee: r.assignee_id.as_ref().and_then(|a| actors.get(a)).cloned(),
            labels: labels_by.remove(&r.id).unwrap_or_default(),
            project: r.project_slug.clone(),
            milestone: r.milestone_name.clone(),
            parent: r.parent_key.as_deref().and_then(|k| k.parse().ok()),
            wanted_executor: r.executor(),
            waiting_on: r.waiting_on.as_deref().and_then(|w| w.parse().ok()),
            description: r.description.clone(),
            description_version: r.description_version,
            claim: claims.get(&r.id).cloned(),
            analyzing: analyzing.get(&r.id).cloned(),
            depth: blockers.depth(&r.id),
            open_blockers: blockers.open_blockers(&r.id),
            cost_usd: costs.remove(&r.id),
            open_questions: open_questions.get(&r.id).copied().unwrap_or(0),
            active_seconds: times.remove(&r.id),
            estimate_minutes: r.estimate_minutes.map(|m| m as u32),
            estimate_cost_usd: r.estimate_cost_usd,
            created_by: r.created_by.as_ref().and_then(|a| actors.get(a)).cloned(),
            created_at: parse_ts(&r.created_at),
            updated_at: parse_ts(&r.updated_at),
            completed_at: parse_ts_opt(r.completed_at.clone()),
            deleted_at: parse_ts_opt(r.deleted_at.clone()),
            score: None,
        };
        by_id.insert(r.id, issue);
    }
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

pub async fn load_one(
    conn: &mut SqliteConnection,
    id: &str,
    with_description: bool,
) -> ApiResult<Issue> {
    load(conn, &[id.to_string()], with_description)
        .await?
        .pop()
        .ok_or_else(|| ApiError::not_found("issue", id))
}

pub async fn load_one_with(
    conn: &mut SqliteConnection,
    id: &str,
    with_description: bool,
    lookup: &Lookup,
) -> ApiResult<Issue> {
    load_with(conn, &[id.to_string()], with_description, lookup)
        .await?
        .pop()
        .ok_or_else(|| ApiError::not_found("issue", id))
}

/// Short forms of issues, by issue ID. It runs no blocker query, so the
/// blocker code can call it.
pub async fn link_map(
    conn: &mut SqliteConnection,
    ids: &[String],
) -> ApiResult<HashMap<String, IssueLink>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let statuses = teams::status_map(conn).await?;
    link_map_with(conn, ids, &statuses).await
}

/// Short forms of issues, by issue ID, with the statuses given.
pub async fn link_map_with(
    conn: &mut SqliteConnection,
    ids: &[String],
    statuses: &HashMap<String, teams::StatusRow>,
) -> ApiResult<HashMap<String, IssueLink>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut out = HashMap::with_capacity(ids.len());
    for r in rows(conn, ids, false).await? {
        let status = statuses
            .get(&r.status_id)
            .map(super::teams::StatusRow::to_ref)
            .ok_or_else(|| ApiError::internal("issue has an unknown status"))?;
        let key = r
            .key
            .parse()
            .map_err(|_| ApiError::internal(format!("bad key {}", r.key)))?;
        out.insert(
            r.id,
            IssueLink {
                key,
                title: r.title,
                status,
                deleted: r.deleted_at.is_some(),
            },
        );
    }
    Ok(out)
}

/// Short forms of issues, in the order of `ids`.
pub async fn links(
    conn: &mut SqliteConnection,
    ids: &[String],
    statuses: &HashMap<String, teams::StatusRow>,
) -> ApiResult<Vec<IssueLink>> {
    let mut map = link_map_with(conn, ids, statuses).await?;
    Ok(ids.iter().filter_map(|id| map.remove(id)).collect())
}

/// Turns free text into an FTS5 query of quoted terms, so punctuation in the
/// text never breaks the query syntax.
pub fn fts_terms(text: &str, min_len: usize, joiner: &str) -> Option<String> {
    let terms: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(|w| w.trim_matches('-'))
        .filter(|w| w.chars().count() >= min_len)
        .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(joiner))
}

const SORT_PRIORITY: &str = "CASE WHEN i.priority = 0 THEN 5 ELSE i.priority END";
/// The text-search score of an issue. Higher is better.
const SCORE: &str = "(SELECT max(score) FROM fts WHERE fts.issue_id = i.id)";

#[derive(Clone, Copy)]
enum KeyKind {
    Int,
    Real,
    Text,
}

/// One sort column of an issue query. The issue ID always breaks ties.
struct SortKey {
    expr: &'static str,
    desc: bool,
    kind: KeyKind,
}

fn sort_keys(ranked: bool, q: &IssueQuery) -> Vec<SortKey> {
    use KeyKind::*;
    let key = |expr, desc, kind| SortKey { expr, desc, kind };
    if q.order == Some(llmtodo_api_types::IssueOrder::Id) {
        Vec::new()
    } else if ranked {
        vec![key(SCORE, true, Real), key("i.updated_at", true, Text)]
    } else if q.trashed {
        vec![key("i.deleted_at", true, Text)]
    } else {
        vec![
            key(SORT_PRIORITY, false, Int),
            key("i.updated_at", true, Text),
        ]
    }
}

/// A keyset cursor: the sort values and the ID of the last issue on a page.
/// The next page starts right after it.
#[derive(serde::Serialize, serde::Deserialize)]
struct Cursor {
    keys: Vec<Value>,
    id: String,
}

impl Cursor {
    fn of(row: &SqliteRow, keys: &[SortKey]) -> ApiResult<Self> {
        let mut values = Vec::with_capacity(keys.len());
        for (n, k) in keys.iter().enumerate() {
            let col = format!("k{n}");
            values.push(match k.kind {
                KeyKind::Int => Value::from(row.try_get::<i64, _>(col.as_str())?),
                KeyKind::Real => Value::from(row.try_get::<f64, _>(col.as_str())?),
                KeyKind::Text => Value::from(row.try_get::<String, _>(col.as_str())?),
            });
        }
        Ok(Cursor {
            keys: values,
            id: row.try_get("id")?,
        })
    }

    fn encode(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    }

    fn decode(s: &str) -> ApiResult<Self> {
        let bad = || ApiError::bad_request("bad cursor");
        let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(s)
            .map_err(|_| bad())?;
        serde_json::from_slice(&json).map_err(|_| bad())
    }
}

/// Pushes the condition "this row sorts after the cursor", one key at a
/// time: a greater first key, or an equal first key and a greater rest.
fn push_after(
    b: &mut QueryBuilder<Sqlite>,
    keys: &[SortKey],
    n: usize,
    c: &Cursor,
) -> ApiResult<()> {
    let Some(k) = keys.get(n) else {
        b.push("id > ").push_bind(c.id.clone());
        return Ok(());
    };
    let op = if k.desc { "<" } else { ">" };
    b.push(format_args!("(k{n} {op} "));
    push_key(b, k.kind, &c.keys[n])?;
    b.push(format_args!(" OR (k{n} = "));
    push_key(b, k.kind, &c.keys[n])?;
    b.push(" AND ");
    push_after(b, keys, n + 1, c)?;
    b.push("))");
    Ok(())
}

fn push_key(b: &mut QueryBuilder<Sqlite>, kind: KeyKind, v: &Value) -> ApiResult<()> {
    let bad = || ApiError::bad_request("bad cursor");
    match kind {
        KeyKind::Int => b.push_bind(v.as_i64().ok_or_else(bad)?),
        KeyKind::Real => b.push_bind(v.as_f64().ok_or_else(bad)?),
        KeyKind::Text => b.push_bind(v.as_str().ok_or_else(bad)?.to_string()),
    };
    Ok(())
}

/// The FTS5 queries for text and duplicate search, when the query has them.
fn search_terms(q: &IssueQuery) -> ApiResult<(Option<String>, Option<String>)> {
    let ranked_text = q.text.as_deref().and_then(|t| fts_terms(t, 1, " "));
    let ranked_similar = q.similar.as_deref().and_then(|t| fts_terms(t, 3, " OR "));
    if q.text.is_some() && ranked_text.is_none() {
        return Err(ApiError::bad_request("text search needs at least one word"));
    }
    if q.similar.is_some() && ranked_similar.is_none() {
        return Err(ApiError::bad_request(
            "similar search needs a word of 3 or more characters",
        ));
    }
    Ok((ranked_text, ranked_similar))
}

/// Pushes the `fts` common table. FTS5 ranks only inside a direct MATCH
/// query, so the matches are collected first. `rank` is lower for better
/// matches.
fn push_matches(b: &mut QueryBuilder<Sqlite>, text: Option<&str>, similar: Option<&str>) {
    if let Some(t) = text {
        b.push("WITH fts AS MATERIALIZED (SELECT issue_id, -rank AS score FROM search_fts WHERE search_fts MATCH ").push_bind(t.to_string()).push(") ");
    } else if let Some(t) = similar {
        b.push("WITH fts AS MATERIALIZED (SELECT issue_id, -rank AS score FROM issues_trgm WHERE issues_trgm MATCH ").push_bind(t.to_string()).push(") ");
    }
}

/// Pushes the start of the two nested selects. The inner query filters. The
/// outer one pages by the sort keys, so a write between two pages never
/// shifts the rows after the cursor.
fn push_select(b: &mut QueryBuilder<Sqlite>, keys: &[SortKey], ranked: bool, trashed: bool) {
    b.push("SELECT id, score");
    for n in 0..keys.len() {
        b.push(format_args!(", k{n}"));
    }
    b.push(" FROM (SELECT i.id AS id, ");
    b.push(if ranked { SCORE } else { "NULL" });
    b.push(" AS score");
    for (n, k) in keys.iter().enumerate() {
        b.push(format_args!(", {} AS k{n}", k.expr));
    }
    b.push(" FROM issues i JOIN statuses s ON s.id = i.status_id WHERE ");
    b.push(if trashed {
        "i.deleted_at IS NOT NULL"
    } else {
        "i.deleted_at IS NULL"
    });
}

/// Pushes every filter of the query, in a fixed order.
async fn push_filters(
    b: &mut QueryBuilder<Sqlite>,
    conn: &mut SqliteConnection,
    me: &Actor,
    q: &IssueQuery,
) -> ApiResult<()> {
    push_key_filter(b, conn, q).await?;
    push_org_filters(b, conn, q).await?;
    filter::push(b, conn, me, &q.filter).await?;
    push_link_filters(b, conn, q).await?;
    push_waiting_on(b, q)?;
    if let Some(d) = &q.depends_on {
        let start = resolve_id(conn, d).await?;
        let ids = deps::dependents(conn, &start).await?;
        b.push(" AND i.id IN (SELECT value FROM json_each(")
            .push_bind(serde_json::to_string(&ids)?)
            .push("))");
    }
    push_field_filters(b, q)?;
    Ok(())
}

/// Keeps only the issues named in `q.keys`. A key that names no issue is
/// skipped, so a list of keys from live events never fails on one that
/// went away.
async fn push_key_filter(
    b: &mut QueryBuilder<Sqlite>,
    conn: &mut SqliteConnection,
    q: &IssueQuery,
) -> ApiResult<()> {
    if q.keys.is_empty() {
        return Ok(());
    }
    let mut ids = Vec::with_capacity(q.keys.len());
    for k in &q.keys {
        match resolve_id(conn, k).await {
            Ok(id) => ids.push(id),
            Err(e) if e.code == llmtodo_api_types::ErrorCode::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    b.push(" AND i.id IN (SELECT value FROM json_each(")
        .push_bind(serde_json::to_string(&ids)?)
        .push("))");
    Ok(())
}

async fn push_org_filters(
    b: &mut QueryBuilder<Sqlite>,
    conn: &mut SqliteConnection,
    q: &IssueQuery,
) -> ApiResult<()> {
    if !q.teams.is_empty() {
        let mut ids = Vec::new();
        for t in &q.teams {
            ids.push(teams::team_row(conn, t).await?.id);
        }
        b.push(" AND i.team_id IN (SELECT value FROM json_each(")
            .push_bind(serde_json::to_string(&ids)?)
            .push("))");
    }
    if !q.projects.is_empty() {
        let mut ids = Vec::new();
        for p in &q.projects {
            ids.push(projects::resolve(conn, p).await?.id);
        }
        b.push(" AND i.project_id IN (SELECT value FROM json_each(")
            .push_bind(serde_json::to_string(&ids)?)
            .push("))");
    }
    Ok(())
}

async fn push_link_filters(
    b: &mut QueryBuilder<Sqlite>,
    conn: &mut SqliteConnection,
    q: &IssueQuery,
) -> ApiResult<()> {
    if let Some(p) = &q.parent {
        if p.eq_ignore_ascii_case("none") {
            b.push(" AND i.parent_id IS NULL");
        } else {
            let id = resolve_id(conn, p).await?;
            b.push(" AND i.parent_id = ").push_bind(id);
        }
    }
    if let Some(m) = &q.milestone {
        b.push(" AND i.milestone_id IN (SELECT m.id FROM milestones m WHERE m.id = ")
            .push_bind(m.clone())
            .push(" OR m.name = ")
            .push_bind(m.clone())
            .push(" COLLATE NOCASE)");
    }
    Ok(())
}

fn push_waiting_on(b: &mut QueryBuilder<Sqlite>, q: &IssueQuery) -> ApiResult<()> {
    if let Some(w) = q.waiting_on.as_deref().map(str::trim) {
        match w.to_ascii_lowercase().as_str() {
            "none" | "" => {
                b.push(" AND i.waiting_on IS NULL");
            }
            "agent" | "owner" => {
                b.push(" AND i.waiting_on = ")
                    .push_bind(w.to_ascii_lowercase());
            }
            other => {
                return Err(ApiError::bad_request(format!(
                    "waiting_on must be agent, owner or none, got {other:?}"
                )));
            }
        }
    }
    Ok(())
}

fn push_field_filters(b: &mut QueryBuilder<Sqlite>, q: &IssueQuery) -> ApiResult<()> {
    if let Some(p) = q.priority {
        b.push(" AND i.priority = ").push_bind(p.as_number() as i64);
    }
    if let Some(t) = q.updated_after {
        b.push(" AND i.updated_at > ").push_bind(crate::db::ts(t));
    }
    if let Some(want) = &q.for_executor {
        // Every filter the issue wants must appear in the query with the
        // same value, ignoring case. This matches
        // `WantedExecutor::satisfied_by`, and runs before paging.
        b.push(" AND NOT EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(i.wanted_executor) THEN i.wanted_executor ELSE '{}' END) w WHERE lower(w.value) IS NOT lower(json_extract(")
            .push_bind(serde_json::to_string(want)?)
            .push(", '$.\"' || w.key || '\"')))");
    }
    Ok(())
}

/// Pushes the cursor condition and the order.
fn push_page(
    b: &mut QueryBuilder<Sqlite>,
    keys: &[SortKey],
    after: Option<&Cursor>,
) -> ApiResult<()> {
    if let Some(c) = after {
        b.push(" WHERE ");
        push_after(b, keys, 0, c)?;
    }
    b.push(" ORDER BY ");
    for (n, k) in keys.iter().enumerate() {
        b.push(format_args!(
            "k{n} {}, ",
            if k.desc { "DESC" } else { "ASC" }
        ));
    }
    b.push("id ASC");
    Ok(())
}

/// Runs an issue query. Returns IDs, scores and the next cursor.
pub async fn query(
    conn: &mut SqliteConnection,
    me: &Actor,
    q: &IssueQuery,
) -> ApiResult<(Vec<(String, Option<f64>)>, Option<String>)> {
    let (mut b, keys, limit) = build_query(conn, me, q, "", true).await?;
    let rows: Vec<SqliteRow> = b.build().fetch_all(&mut *conn).await?;
    let more = rows.len() as i64 > limit;
    let rows = &rows[..rows.len().min(limit as usize)];
    let next = match rows.last() {
        Some(last) if more => Some(Cursor::of(last, &keys)?.encode()),
        _ => None,
    };
    let mut found: Vec<(String, Option<f64>)> = Vec::with_capacity(rows.len());
    for r in rows {
        found.push((r.try_get("id")?, r.try_get("score")?));
    }
    Ok((found, next))
}

/// The IDs of every issue a query matches, in the query's order, in one SQL
/// statement. It reads no cursor and no limit.
pub async fn query_ids(
    conn: &mut SqliteConnection,
    me: &Actor,
    q: &IssueQuery,
) -> ApiResult<Vec<String>> {
    let q = IssueQuery {
        cursor: None,
        limit: None,
        ..q.clone()
    };
    let (mut b, _, _) = build_query(conn, me, &q, "", false).await?;
    let rows: Vec<SqliteRow> = b.build().fetch_all(&mut *conn).await?;
    rows.iter()
        .map(|r| r.try_get("id").map_err(Into::into))
        .collect()
}

/// Builds the SQL of an issue query after `prefix`. Returns it with the sort
/// keys and the page size. Without `paged`, the SQL has no limit.
async fn build_query(
    conn: &mut SqliteConnection,
    me: &Actor,
    q: &IssueQuery,
    prefix: &str,
    paged: bool,
) -> ApiResult<(QueryBuilder<Sqlite>, Vec<SortKey>, i64)> {
    let (ranked_text, ranked_similar) = search_terms(q)?;
    let mut b: QueryBuilder<Sqlite> = QueryBuilder::new(prefix);
    let ranked = ranked_text.is_some() || ranked_similar.is_some();
    push_matches(&mut b, ranked_text.as_deref(), ranked_similar.as_deref());
    let keys = sort_keys(ranked, q);
    let after = q.cursor.as_deref().map(Cursor::decode).transpose()?;
    if let Some(c) = &after
        && c.keys.len() != keys.len()
    {
        return Err(ApiError::bad_request("bad cursor"));
    }
    push_select(&mut b, &keys, ranked, q.trashed);
    push_filters(&mut b, conn, me, q).await?;
    if ranked {
        b.push(" AND i.id IN (SELECT issue_id FROM fts)");
    }
    b.push(") AS q");
    push_page(&mut b, &keys, after.as_ref())?;
    let limit = q
        .limit
        .unwrap_or(100)
        .clamp(1, llmtodo_api_types::MAX_ISSUE_PAGE) as i64;
    if paged {
        b.push(" LIMIT ").push_bind(limit + 1);
    }
    Ok((b, keys, limit))
}

async fn executor_ok(
    conn: &mut SqliteConnection,
    ids: &[String],
    want: &WantedExecutor,
) -> ApiResult<std::collections::HashSet<String>> {
    let json = serde_json::to_string(ids)?;
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, wanted_executor FROM issues WHERE id IN (SELECT value FROM json_each(?))",
    )
    .bind(json)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|(_, w)| {
            serde_json::from_str::<WantedExecutor>(w)
                .unwrap_or_default()
                .satisfied_by(want)
        })
        .map(|(id, _)| id)
        .collect())
}

/// Open, live, unassigned issues with no open blocker, under a map issue
/// (to `depth` levels) or in a project. An issue in a review status waits
/// for the owner, so it shows only when it waits on an agent, as after an
/// approval with an open PR. Sorted by priority, then key.
pub async fn frontier(
    conn: &mut SqliteConnection,
    root: Option<&str>,
    project: Option<&str>,
    depth: u32,
    want: Option<&WantedExecutor>,
) -> ApiResult<Vec<String>> {
    let base = "SELECT i.id FROM issues i JOIN statuses s ON s.id = i.status_id JOIN teams t ON t.id = i.team_id
        WHERE i.deleted_at IS NULL AND i.assignee_id IS NULL AND s.type NOT IN ('completed', 'canceled')
        AND i.waiting_on IS NOT 'owner'
        AND (s.review = 0 OR i.waiting_on IS 'agent')
        AND NOT EXISTS (
            SELECT 1 FROM relations r JOIN issues b ON b.id = r.from_id JOIN statuses bs ON bs.id = b.status_id
            WHERE r.type = 'blocks' AND r.to_id = i.id AND b.deleted_at IS NULL AND bs.type NOT IN ('completed', 'canceled')
        )";
    let order = format!(" ORDER BY {SORT_PRIORITY}, t.key, i.number");
    let ids: Vec<String> = match (root, project) {
        (Some(root), _) => {
            let sql = format!(
                "WITH RECURSIVE tree(id, depth) AS (
                    SELECT id, 1 FROM issues WHERE parent_id = ?
                    UNION ALL
                    SELECT c.id, tree.depth + 1 FROM issues c JOIN tree ON c.parent_id = tree.id WHERE tree.depth < ?
                 ) {base} AND i.id IN (SELECT id FROM tree){order}"
            );
            sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
                .bind(root)
                .bind(depth.max(1) as i64)
                .fetch_all(&mut *conn)
                .await?
        }
        (None, Some(project)) => {
            let sql = format!("{base} AND i.project_id = ?{order}");
            sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
                .bind(project)
                .fetch_all(&mut *conn)
                .await?
        }
        (None, None) => {
            return Err(ApiError::bad_request(
                "frontier needs a map issue or a project",
            ));
        }
    };
    match want {
        None => Ok(ids),
        Some(w) => {
            let ok = executor_ok(conn, &ids, w).await?;
            Ok(ids.into_iter().filter(|id| ok.contains(id)).collect())
        }
    }
}

/// True when `ancestor` is `id` or one of its parents.
pub async fn is_ancestor(conn: &mut SqliteConnection, ancestor: &str, id: &str) -> ApiResult<bool> {
    let found: Option<i64> = sqlx::query_scalar(
        "WITH RECURSIVE up(id) AS (SELECT ? UNION SELECT i.parent_id FROM issues i JOIN up ON i.id = up.id WHERE i.parent_id IS NOT NULL)
         SELECT 1 FROM up WHERE id = ? LIMIT 1",
    )
    .bind(id)
    .bind(ancestor)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(found.is_some())
}

pub async fn touch_updated(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET updated_at = ? WHERE id = ?")
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    c: &mut SqliteConnection,
    id: &str,
    team_id: &str,
    number: i64,
    key: &str,
    title: &str,
    description: &str,
    status_id: &str,
    priority: i64,
    actor_id: String,
    t: &str,
    completed: Option<String>,
    executor: String,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO issues (id, team_id, number, key, title, description, status_id, priority, created_by, created_at, updated_at, completed_at, wanted_executor)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
        .bind(id)
        .bind(team_id)
        .bind(number)
        .bind(key)
        .bind(title)
        .bind(description)
        .bind(status_id)
        .bind(priority)
        .bind(actor_id)
        .bind(t)
        .bind(t)
        .bind(completed)
        .bind(executor)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn add_key(c: &mut SqliteConnection, key: &str, id: &str) -> ApiResult<()> {
    sqlx::query("INSERT INTO issue_keys (key, issue_id) VALUES (?, ?)")
        .bind(key)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn git_fields(
    c: &mut SqliteConnection,
    id: &str,
) -> ApiResult<(
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
)> {
    Ok(
        sqlx::query_as(
            "SELECT git_branch, git_worktree, git_host, git_pr FROM issues WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&mut *c)
        .await?,
    )
}

pub async fn set_git_branch(
    c: &mut SqliteConnection,
    branch: Option<&str>,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET git_branch = ? WHERE id = ?")
        .bind(branch)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_git_worktree(
    c: &mut SqliteConnection,
    worktree: Option<&str>,
    host: Option<&str>,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET git_worktree = ?, git_host = ? WHERE id = ?")
        .bind(worktree)
        .bind(host)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_git_pr(c: &mut SqliteConnection, pr: Option<&str>, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET git_pr = ? WHERE id = ?")
        .bind(pr)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_title(c: &mut SqliteConnection, title: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET title = ? WHERE id = ?")
        .bind(title)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_priority(c: &mut SqliteConnection, priority: i64, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET priority = ? WHERE id = ?")
        .bind(priority)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_waiting_on(
    c: &mut SqliteConnection,
    waiting_on: Option<&str>,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET waiting_on = ? WHERE id = ?")
        .bind(waiting_on)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_executor(c: &mut SqliteConnection, executor: String, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET wanted_executor = ? WHERE id = ?")
        .bind(executor)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_project(
    c: &mut SqliteConnection,
    project_id: Option<String>,
    keep_milestone: bool,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET project_id = ?, milestone_id = CASE WHEN ? THEN milestone_id ELSE NULL END WHERE id = ?")
        .bind(project_id)
        .bind(keep_milestone)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_milestone(
    c: &mut SqliteConnection,
    milestone_id: Option<String>,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET milestone_id = ? WHERE id = ?")
        .bind(milestone_id)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_parent(
    c: &mut SqliteConnection,
    parent_id: Option<String>,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET parent_id = ? WHERE id = ?")
        .bind(parent_id)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn description(c: &mut SqliteConnection, id: &str) -> ApiResult<String> {
    Ok(
        sqlx::query_scalar("SELECT description FROM issues WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *c)
            .await?,
    )
}

pub async fn assignee(c: &mut SqliteConnection, id: &str) -> ApiResult<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT assignee_id FROM issues WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *c)
            .await?,
    )
}

pub async fn set_assignee(
    c: &mut SqliteConnection,
    assignee_id: Option<&str>,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET assignee_id = ? WHERE id = ?")
        .bind(assignee_id)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn status_id(c: &mut SqliteConnection, id: &str) -> ApiResult<String> {
    Ok(
        sqlx::query_scalar("SELECT status_id FROM issues WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *c)
            .await?,
    )
}

pub async fn waiting_on(c: &mut SqliteConnection, id: &str) -> ApiResult<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT waiting_on FROM issues WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *c)
            .await?,
    )
}

pub async fn clear_waiting_on(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET waiting_on = NULL WHERE id = ?")
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_status(
    c: &mut SqliteConnection,
    status_id: &str,
    completed: Option<String>,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET status_id = ?, completed_at = ? WHERE id = ?")
        .bind(status_id)
        .bind(completed)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn description_and_waiting_on(
    c: &mut SqliteConnection,
    id: &str,
) -> ApiResult<(String, Option<String>)> {
    Ok(
        sqlx::query_as("SELECT description, waiting_on FROM issues WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *c)
            .await?,
    )
}

pub async fn hand_to_agent(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET waiting_on = 'agent' WHERE id = ?")
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn description_and_version(
    c: &mut SqliteConnection,
    id: &str,
) -> ApiResult<(String, i64)> {
    Ok(
        sqlx::query_as("SELECT description, description_version FROM issues WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *c)
            .await?,
    )
}

pub async fn set_description(
    c: &mut SqliteConnection,
    text: &str,
    version: i64,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET description = ?, description_version = ? WHERE id = ?")
        .bind(text)
        .bind(version)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn move_team(
    c: &mut SqliteConnection,
    team_id: &str,
    number: i64,
    key: &str,
    status_id: &str,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET team_id = ?, number = ?, key = ?, status_id = ? WHERE id = ?")
        .bind(team_id)
        .bind(number)
        .bind(key)
        .bind(status_id)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn trash(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET deleted_at = ?, updated_at = ? WHERE id = ?")
        .bind(now())
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn untrash(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET deleted_at = NULL, updated_at = ? WHERE id = ?")
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn merge_claim_git(
    c: &mut SqliteConnection,
    branch: Option<&str>,
    worktree: Option<&str>,
    host: Option<&str>,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET git_branch = coalesce(?, git_branch), git_worktree = coalesce(?, git_worktree), git_host = CASE WHEN ? IS NULL THEN git_host ELSE ? END WHERE id = ?")
        .bind(branch)
        .bind(worktree)
        .bind(worktree)
        .bind(host)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn clear_parent(c: &mut SqliteConnection, parent_id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET parent_id = NULL WHERE parent_id = ?")
        .bind(parent_id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn delete_if_trashed_before(
    c: &mut SqliteConnection,
    id: &str,
    before: Option<&str>,
) -> ApiResult<()> {
    sqlx::query("DELETE FROM issues WHERE id = ? AND deleted_at IS NOT NULL AND deleted_at < ?")
        .bind(id)
        .bind(before)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn trashed_before(
    c: &mut SqliteConnection,
    before: String,
) -> ApiResult<Vec<(String, String)>> {
    Ok(sqlx::query_as(
        "SELECT id, key FROM issues WHERE deleted_at IS NOT NULL AND deleted_at < ? ORDER BY key",
    )
    .bind(before)
    .fetch_all(&mut *c)
    .await?)
}

pub async fn count_all(c: &mut SqliteConnection) -> ApiResult<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM issues")
        .fetch_one(&mut *c)
        .await?)
}

pub async fn set_estimate_minutes(
    c: &mut SqliteConnection,
    minutes: u32,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET estimate_minutes = ? WHERE id = ?")
        .bind(i64::from(minutes))
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_estimate_cost(c: &mut SqliteConnection, usd: f64, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET estimate_cost_usd = ? WHERE id = ?")
        .bind(usd)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use llmtodo_api_types::IssueQuery;
    use sqlx::Row as _;

    async fn plan(
        c: &mut sqlx::SqliteConnection,
        me: &llmtodo_api_types::Actor,
        q: IssueQuery,
    ) -> String {
        let (mut b, _, _) = super::build_query(c, me, &q, "EXPLAIN QUERY PLAN ", true)
            .await
            .unwrap();
        let rows = b.build().fetch_all(&mut *c).await.unwrap();
        rows.iter()
            .map(|r| r.get::<String, _>("detail"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// LLM-180: the default sort reads the issues in index order, with no
    /// scan of the whole table and no sort, on every page.
    #[tokio::test]
    async fn the_default_sort_uses_its_index() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        crate::store::actors::seed(&pool).await.unwrap();
        let me = crate::store::actors::owner(&pool).await.unwrap();
        let mut c = pool.acquire().await.unwrap();
        let first = plan(&mut c, &me, IssueQuery::default()).await;
        assert!(first.contains("USING INDEX issues_default_sort"), "{first}");
        assert!(!first.contains("TEMP B-TREE"), "{first}");
        let cursor = super::Cursor {
            keys: vec![2.into(), "2026-11-12T00:00:00Z".into()],
            id: "x".into(),
        };
        let next = plan(
            &mut c,
            &me,
            IssueQuery {
                cursor: Some(cursor.encode()),
                ..Default::default()
            },
        )
        .await;
        assert!(next.contains("USING INDEX issues_default_sort"), "{next}");
        assert!(!next.contains("TEMP B-TREE"), "{next}");
    }

    #[test]
    fn fts_terms_quote_words() {
        assert_eq!(
            super::fts_terms("fix: \"login\" bug", 1, " ").unwrap(),
            "\"fix\" \"login\" \"bug\""
        );
        assert_eq!(
            super::fts_terms("a bc defg", 3, " OR ").unwrap(),
            "\"defg\""
        );
        assert!(super::fts_terms("  ", 1, " ").is_none());
    }
}
