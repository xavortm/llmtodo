//! `llmtodo import linear`: copies a Linear workspace into llmtodo.
//!
//! The import is idempotent. Every imported row stores its Linear ID, and a
//! second run updates rows instead of adding new ones. Issues keep their
//! Linear keys, such as `ALX-123`.
//!
//! It reads everything from Linear first, then downloads images, and only
//! then opens one write transaction. A dry run makes the same changes and
//! rolls them back, so its counts are exact.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::time::Duration;

use llmtodo_api_types::{
    ActivityField, ClientContext, EntityType, ImportReport, LinearImportRequest,
};
use serde_json::{Value, json};
use sha2::Digest;

use crate::db::{self, new_id, now};
use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, Ctx, Write};
use crate::store::{self, actors, projects};

pub const LINEAR_API: &str = "https://api.linear.app/graphql";

/// Where the importer gets Linear data. Tests use recorded responses.
pub trait Fetch: Send + Sync {
    fn graphql(
        &self,
        query: &str,
        variables: Value,
    ) -> impl Future<Output = ApiResult<Value>> + Send;
    fn download(
        &self,
        url: &str,
    ) -> impl Future<Output = ApiResult<(Vec<u8>, Option<String>)>> + Send;
}

pub struct LinearHttp {
    token: String,
    http: reqwest::Client,
    pub requests: std::sync::atomic::AtomicU64,
}

impl LinearHttp {
    pub fn new(token: &str) -> Self {
        Self {
            token: token.trim().to_string(),
            http: reqwest::Client::new(),
            requests: Default::default(),
        }
    }
}

impl Fetch for LinearHttp {
    async fn graphql(&self, query: &str, variables: Value) -> ApiResult<Value> {
        for attempt in 0..8 {
            self.requests
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let res = self
                .http
                .post(LINEAR_API)
                .header("Authorization", &self.token)
                .json(&json!({ "query": query, "variables": variables }))
                .send()
                .await
                .map_err(|e| ApiError::internal(format!("Linear request failed: {e}")))?;
            let status = res.status().as_u16();
            let reset = res
                .headers()
                .get("x-ratelimit-requests-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok());
            let body: Value = res.json().await.unwrap_or(Value::Null);
            let limited = status == 429
                || body["errors"]
                    .as_array()
                    .is_some_and(|e| e.iter().any(|e| e["extensions"]["code"] == "RATELIMITED"));
            if limited {
                let wait_ms = reset
                    .map(|r| r - jiff::Timestamp::now().as_millisecond())
                    .unwrap_or(30_000)
                    .clamp(1_000, 3_600_000);
                tracing::warn!(wait_ms, attempt, "Linear rate limit; waiting");
                tokio::time::sleep(Duration::from_millis(wait_ms as u64)).await;
                continue;
            }
            if status == 401 || status == 403 {
                return Err(ApiError::forbidden(
                    "Linear refused the API key; check LINEAR_API_KEY",
                ));
            }
            if let Some(errors) = body["errors"].as_array().filter(|e| !e.is_empty()) {
                return Err(ApiError::internal(format!(
                    "Linear GraphQL error: {}",
                    Value::Array(errors.clone())
                )));
            }
            if status >= 400 {
                return Err(ApiError::internal(format!(
                    "Linear returned HTTP {status}: {body}"
                )));
            }
            return Ok(body["data"].clone());
        }
        Err(ApiError::internal(
            "Linear kept rate-limiting the import; try again later",
        ))
    }

    async fn download(&self, url: &str) -> ApiResult<(Vec<u8>, Option<String>)> {
        self.requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let res = self
            .http
            .get(url)
            .header("Authorization", &self.token)
            .send()
            .await
            .map_err(|e| ApiError::internal(format!("download {url}: {e}")))?;
        if !res.status().is_success() {
            return Err(ApiError::internal(format!(
                "download {url}: HTTP {}",
                res.status()
            )));
        }
        let mime = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = res
            .bytes()
            .await
            .map_err(|e| ApiError::internal(format!("download {url}: {e}")))?;
        Ok((bytes.to_vec(), mime))
    }
}

// ---------------------------------------------------------------- queries

const META: &str = "query Meta {
  viewer { id name displayName email }
  teams(first: 25) { nodes { id key name states(first: 50) { nodes { id name type color position } } } }
}";

const LABELS: &str = "query Labels($after: String, $first: Int!) {
  issueLabels(first: $first, after: $after) { nodes { id name color description isGroup parent { id } } pageInfo { hasNextPage endCursor } }
}";

const USERS: &str = "query Users($after: String, $first: Int!) {
  users(first: $first, after: $after, includeArchived: true) { nodes { id name displayName email } pageInfo { hasNextPage endCursor } }
}";

const PROJECTS: &str = "query Projects($after: String, $first: Int!) {
  projects(first: $first, after: $after, includeArchived: true) {
    nodes { id name description createdAt updatedAt archivedAt status { type } teams(first: 20) { nodes { id } }
      projectMilestones(first: 100) { nodes { id name description targetDate sortOrder } } }
    pageInfo { hasNextPage endCursor }
  }
}";

const DOCUMENTS: &str = "query Documents($after: String, $first: Int!) {
  documents(first: $first, after: $after, includeArchived: true) {
    nodes { id title content slugId createdAt updatedAt archivedAt project { id } }
    pageInfo { hasNextPage endCursor }
  }
}";

const ISSUES: &str = "query Issues($after: String, $first: Int!, $filter: IssueFilter) {
  issues(first: $first, after: $after, includeArchived: true, filter: $filter) {
    nodes { id identifier number title description priority createdAt updatedAt archivedAt completedAt canceledAt
      state { id } team { id } project { id } projectMilestone { id } parent { id } assignee { id } creator { id }
      labels(first: 30) { nodes { id } pageInfo { hasNextPage endCursor } }
      relations(first: 50) { nodes { id type relatedIssue { id } } pageInfo { hasNextPage endCursor } }
      attachments(first: 50) { nodes { id url title subtitle metadata createdAt sourceType } pageInfo { hasNextPage endCursor } }
      comments(first: 100) { nodes { id body createdAt updatedAt user { id } parent { id } } pageInfo { hasNextPage endCursor } }
    }
    pageInfo { hasNextPage endCursor }
  }
}";

/// The nested lists on an issue. `ISSUES` reads only their first page; each
/// query here reads the rest of one list for one issue.
const ISSUE_LISTS: [(&str, &str); 4] = [
    (
        "labels",
        "query IssueLabels($id: String!, $after: String, $first: Int!) {
  issue(id: $id) { labels(first: $first, after: $after) { nodes { id } pageInfo { hasNextPage endCursor } } }
}",
    ),
    (
        "relations",
        "query IssueRelations($id: String!, $after: String, $first: Int!) {
  issue(id: $id) { relations(first: $first, after: $after) { nodes { id type relatedIssue { id } } pageInfo { hasNextPage endCursor } } }
}",
    ),
    (
        "attachments",
        "query IssueAttachments($id: String!, $after: String, $first: Int!) {
  issue(id: $id) { attachments(first: $first, after: $after) { nodes { id url title subtitle metadata createdAt sourceType } pageInfo { hasNextPage endCursor } } }
}",
    ),
    (
        "comments",
        "query IssueComments($id: String!, $after: String, $first: Int!) {
  issue(id: $id) { comments(first: $first, after: $after) { nodes { id body createdAt updatedAt user { id } parent { id } } pageInfo { hasNextPage endCursor } } }
}",
    ),
];

/// Reads the pages of each nested list that `ISSUES` left out, so a full
/// list never drops rows without a word.
async fn read_nested_lists(fetch: &impl Fetch, issues: &mut [Value]) -> ApiResult<()> {
    for issue in issues {
        for (field, query) in ISSUE_LISTS {
            let info = &issue[field]["pageInfo"];
            if info["hasNextPage"].as_bool() != Some(true) {
                continue;
            }
            let vars = json!({ "id": issue["id"], "after": info["endCursor"] });
            let rest = all_pages(fetch, query, &["issue", field], vars).await?;
            let list = &mut issue[field];
            match list["nodes"].as_array_mut() {
                Some(nodes) => nodes.extend(rest),
                None => list["nodes"] = Value::Array(rest),
            }
            list["pageInfo"] = json!({ "hasNextPage": false, "endCursor": null });
        }
    }
    Ok(())
}

async fn all_pages(
    fetch: &impl Fetch,
    query: &str,
    path: &[&str],
    mut vars: Value,
) -> ApiResult<Vec<Value>> {
    let mut out = Vec::new();
    // Big pages can pass Linear's query complexity limit; halve and retry.
    let mut first: i64 = 100;
    loop {
        vars["first"] = json!(first);
        let data = match fetch.graphql(query, vars.clone()).await {
            Ok(d) => d,
            Err(e) if first > 2 && e.message.contains("complex") => {
                first /= 2;
                continue;
            }
            Err(e) => return Err(e),
        };
        let conn = path.iter().fold(&data, |v, k| &v[*k]);
        out.extend(conn["nodes"].as_array().cloned().unwrap_or_default());
        if conn["pageInfo"]["hasNextPage"].as_bool() == Some(true) {
            vars["after"] = conn["pageInfo"]["endCursor"].clone();
        } else {
            return Ok(out);
        }
    }
}

/// Everything read from Linear, before any write.
#[derive(Default)]
pub struct Snapshot {
    pub viewer: Value,
    pub teams: Vec<Value>,
    pub labels: Vec<Value>,
    pub users: Vec<Value>,
    pub projects: Vec<Value>,
    pub documents: Vec<Value>,
    pub issues: Vec<Value>,
}

pub async fn read_linear(fetch: &impl Fetch, team_keys: &[String]) -> ApiResult<Snapshot> {
    let meta = fetch.graphql(META, json!({})).await?;
    let mut teams: Vec<Value> = meta["teams"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !team_keys.is_empty() {
        let want: HashSet<String> = team_keys.iter().map(|k| k.to_ascii_uppercase()).collect();
        teams.retain(|t| {
            t["key"]
                .as_str()
                .is_some_and(|k| want.contains(&k.to_ascii_uppercase()))
        });
        if teams.is_empty() {
            return Err(ApiError::not_found("Linear team", &team_keys.join(",")));
        }
    }
    let keys: Vec<Value> = teams.iter().map(|t| t["key"].clone()).collect();
    let mut issues = Vec::new();
    for key in &keys {
        // Big pages can exceed Linear's query complexity limit; halve and retry.
        let mut first = 50;
        let mut after = Value::Null;
        loop {
            let vars = json!({ "first": first, "after": after, "filter": { "team": { "key": { "eq": key } } } });
            match fetch.graphql(ISSUES, vars).await {
                Ok(data) => {
                    let conn = &data["issues"];
                    issues.extend(conn["nodes"].as_array().cloned().unwrap_or_default());
                    if conn["pageInfo"]["hasNextPage"].as_bool() == Some(true) {
                        after = conn["pageInfo"]["endCursor"].clone();
                    } else {
                        break;
                    }
                }
                Err(e) if first > 2 && e.message.contains("complex") => first /= 2,
                Err(e) => return Err(e),
            }
        }
    }
    read_nested_lists(fetch, &mut issues).await?;
    let team_ids: HashSet<&str> = teams.iter().filter_map(|t| t["id"].as_str()).collect();
    let mut projects = all_pages(fetch, PROJECTS, &["projects"], json!({})).await?;
    if !team_keys.is_empty() {
        projects.retain(|p| {
            p["teams"]["nodes"].as_array().is_some_and(|ts| {
                ts.iter()
                    .any(|t| t["id"].as_str().is_some_and(|id| team_ids.contains(id)))
            })
        });
    }
    let project_ids: HashSet<String> = projects
        .iter()
        .filter_map(|p| p["id"].as_str().map(str::to_string))
        .collect();
    let mut documents = all_pages(fetch, DOCUMENTS, &["documents"], json!({})).await?;
    documents.retain(|d| {
        d["project"]["id"]
            .as_str()
            .is_some_and(|id| project_ids.contains(id))
    });
    Ok(Snapshot {
        viewer: meta["viewer"].clone(),
        teams,
        labels: all_pages(fetch, LABELS, &["issueLabels"], json!({})).await?,
        users: all_pages(fetch, USERS, &["users"], json!({})).await?,
        projects,
        documents,
        issues,
    })
}

// ---------------------------------------------------------------- files

/// Linear-hosted uploads need a Linear login and stop working after the
/// cutover, so they are copied into `files/`.
pub fn linear_upload_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("https://uploads.linear.app/") {
        let tail = &rest[at..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | '"' | '\'' | '>' | '<'))
            .unwrap_or(tail.len());
        out.push(tail[..end].to_string());
        rest = &tail[end..];
    }
    out
}

pub fn file_url(hash: &str) -> String {
    format!("/v1/files/{hash}")
}

fn rewrite(text: &str, files: &HashMap<String, String>) -> String {
    let mut out = text.to_string();
    for (url, hash) in files {
        out = out.replace(url.as_str(), &file_url(hash));
    }
    out
}

async fn download_files(
    st: &AppState,
    fetch: &impl Fetch,
    snap: &Snapshot,
    report: &mut ImportReport,
) -> ApiResult<HashMap<String, String>> {
    let mut texts: Vec<&str> = Vec::new();
    for i in &snap.issues {
        texts.extend(i["description"].as_str());
        for c in i["comments"]["nodes"].as_array().into_iter().flatten() {
            texts.extend(c["body"].as_str());
        }
    }
    for d in &snap.documents {
        texts.extend(d["content"].as_str());
    }
    let urls: std::collections::BTreeSet<String> =
        texts.iter().flat_map(|t| linear_upload_urls(t)).collect();
    let dir = st.data_dir.join("files");
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
    let mut map = HashMap::new();
    let pool = st.pool();
    for url in urls {
        let known: Option<String> =
            store::import::source_hash(&mut *pool.acquire().await?, &url).await?;
        if let Some(h) = known.filter(|h| dir.join(h).exists()) {
            count(report, "files", Change::Unchanged);
            map.insert(url, h);
            continue;
        }
        match fetch.download(&url).await {
            Ok((bytes, mime)) => {
                let hash = hex::encode(sha2::Sha256::digest(&bytes));
                let path = dir.join(&hash);
                if !path.exists() {
                    std::fs::write(&path, &bytes).map_err(|e| ApiError::internal(e.to_string()))?;
                }
                store::import::insert_file(
                    &mut *pool.acquire().await?,
                    &hash,
                    mime,
                    bytes.len() as i64,
                    &url,
                )
                .await?;
                store::import::link_file_source(&mut *pool.acquire().await?, &url, &hash).await?;
                count(report, "files", Change::Created);
                map.insert(url, hash);
            }
            Err(e) => report
                .problems
                .push(format!("could not copy {url}: {}", e.message)),
        }
    }
    Ok(map)
}

// ---------------------------------------------------------------- writes

#[derive(Clone, Copy, PartialEq)]
enum Change {
    Created,
    Updated,
    Unchanged,
}

fn count(report: &mut ImportReport, kind: &str, c: Change) {
    let e = report.counts.entry(kind.to_string()).or_default();
    match c {
        Change::Created => e.created += 1,
        Change::Updated => e.updated += 1,
        Change::Unchanged => e.unchanged += 1,
    }
}

fn s(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string).filter(|s| !s.is_empty())
}

fn t(v: &Value) -> Option<String> {
    v.as_str()
        .and_then(|s| s.parse::<jiff::Timestamp>().ok())
        .map(db::ts)
}

/// Linear status types: triage, backlog, unstarted, started, completed,
/// canceled. Triage has no twin here, so it maps to backlog.
fn status_type(linear: &str) -> &'static str {
    match linear {
        "unstarted" => "unstarted",
        "started" => "started",
        "completed" => "completed",
        "canceled" => "canceled",
        _ => "backlog",
    }
}

fn project_state(linear: &str) -> &'static str {
    match linear {
        "planned" => "planned",
        "started" => "started",
        "paused" => "paused",
        "completed" => "completed",
        "canceled" => "canceled",
        _ => "backlog",
    }
}

fn hex_color(c: &Value) -> String {
    let c = c.as_str().unwrap_or("#8b8d98").to_lowercase();
    if c.len() == 7 && c.starts_with('#') {
        c
    } else {
        "#8b8d98".into()
    }
}

async fn find(w: &mut Write<'_>, table: &str, linear_id: &str) -> ApiResult<Option<String>> {
    store::import::find_linear(w.c(), table, linear_id).await
}

/// Runs the import inside one transaction.
pub async fn apply(
    st: &AppState,
    snap: &Snapshot,
    files: &HashMap<String, String>,
    dry_run: bool,
    report: &mut ImportReport,
) -> ApiResult<()> {
    let actor = actors::system(&st.pool(), "linear-import").await?;
    let ctx = Ctx {
        client: ClientContext {
            session_id: Some(format!(
                "linear-import-{}",
                jiff::Timestamp::now().as_second()
            )),
            ..Default::default()
        },
        actor,
    };
    let mut w = Write::begin(st, &ctx).await?;
    let mut m: HashMap<String, String> = HashMap::new();

    import_actors(&mut w, snap, &mut m, report).await?;
    for team in &snap.teams {
        import_team(&mut w, team, &mut m, report).await?;
    }
    import_labels(&mut w, snap, &mut m, report).await?;
    for p in &snap.projects {
        import_project(&mut w, p, &mut m, report).await?;
    }
    for d in &snap.documents {
        import_document(&mut w, &ctx, d, &m, files, report).await?;
    }
    let kept = import_issues(&mut w, &ctx, snap, &mut m, files, report).await?;
    // Parents and relations, once every issue exists.
    for i in &snap.issues {
        link_issue(&mut w, &ctx, i, &m, &kept, report).await?;
    }

    if dry_run {
        // Dropping the transaction rolls it back.
        drop(w);
    } else {
        w.commit().await?;
    }
    Ok(())
}

/// Imports the users. The key's own user is the owner.
async fn import_actors(
    w: &mut Write<'_>,
    snap: &Snapshot,
    m: &mut HashMap<String, String>,
    report: &mut ImportReport,
) -> ApiResult<()> {
    let viewer_id = s(&snap.viewer["id"]).unwrap_or_default();
    for u in &snap.users {
        let lid = s(&u["id"]).unwrap_or_default();
        let id = if lid == viewer_id {
            link_owner(w, &lid, report).await?
        } else {
            import_actor(w, u, &lid, report).await?
        };
        m.insert(lid, id);
    }
    Ok(())
}

/// Stores the Linear ID of the key's own user on the owner, and returns the
/// owner's ID.
async fn link_owner(w: &mut Write<'_>, lid: &str, report: &mut ImportReport) -> ApiResult<String> {
    let owner: String = store::import::owner_id(w.c()).await?;
    let had: Option<String> = store::import::actor_linear_id(w.c(), &owner).await?;
    if had.as_deref() != Some(lid) {
        store::import::clear_actor_linear_id(w.c(), lid).await?;
        store::import::set_actor_linear_id(w.c(), lid, &owner).await?;
        count(report, "actors", Change::Updated);
    } else {
        count(report, "actors", Change::Unchanged);
    }
    Ok(owner)
}

/// Creates or adopts the actor for one Linear user other than the owner.
async fn import_actor(
    w: &mut Write<'_>,
    u: &Value,
    lid: &str,
    report: &mut ImportReport,
) -> ApiResult<String> {
    let name = s(&u["displayName"])
        .or_else(|| s(&u["name"]))
        .unwrap_or_else(|| "linear-user".into());
    let found = match find(w, "actors", lid).await? {
        Some(id) => Some(id),
        None => store::import::find_human_unlinked(w.c(), &name).await?,
    };
    let id = match found {
        Some(id) => {
            store::import::set_actor_linear_id(w.c(), lid, &id).await?;
            count(report, "actors", Change::Unchanged);
            id
        }
        None => {
            let id = new_id();
            let unique: Option<i64> = store::import::human_name_exists(w.c(), &name).await?;
            let name = if unique.is_some() {
                format!("{name} ({})", &lid[..lid.len().min(8)])
            } else {
                name
            };
            store::import::insert_actor(w.c(), &id, &name, lid).await?;
            count(report, "actors", Change::Created);
            id
        }
    };
    Ok(id)
}

/// Imports one team and its statuses.
async fn import_team(
    w: &mut Write<'_>,
    team: &Value,
    m: &mut HashMap<String, String>,
    report: &mut ImportReport,
) -> ApiResult<()> {
    let lid = s(&team["id"]).unwrap_or_default();
    let key = s(&team["key"]).unwrap_or_default();
    let name = s(&team["name"]).unwrap_or_else(|| key.clone());
    let existing = match find(w, "teams", &lid).await? {
        Some(id) => Some(id),
        None => store::import::team_by_key(w.c(), &key).await?,
    };
    let team_id = match existing {
        Some(id) => {
            store::import::update_team(w.c(), &lid, &name, &id).await?;
            count(report, "teams", Change::Unchanged);
            id
        }
        None => {
            let id = crate::store::teams::create(w.c(), &key, &name, Some(&lid)).await?;
            count(report, "teams", Change::Created);
            id
        }
    };
    m.insert(lid, team_id.clone());
    for state in team["states"]["nodes"].as_array().into_iter().flatten() {
        import_status(w, state, &team_id, &key, m, report).await?;
    }
    Ok(())
}

async fn import_status(
    w: &mut Write<'_>,
    state: &Value,
    team_id: &str,
    key: &str,
    m: &mut HashMap<String, String>,
    report: &mut ImportReport,
) -> ApiResult<()> {
    let sid = s(&state["id"]).unwrap_or_default();
    let sname = s(&state["name"]).unwrap_or_else(|| "Status".into());
    let kind = status_type(state["type"].as_str().unwrap_or(""));
    let pos = state["position"].as_f64().unwrap_or(0.0).round() as i64;
    let found = match find(w, "statuses", &sid).await? {
        Some(id) => Some(id),
        None => store::import::find_status(w.c(), team_id, &sname).await?,
    };
    let id = match found {
        Some(id) => {
            let cur: (String, String) = store::import::status_name_type(w.c(), &id).await?;
            if cur.1 != kind {
                report.problems.push(format!(
                    "status {sname:?} of team {key} is {} here and {kind} in Linear; kept {}",
                    cur.1, cur.1
                ));
            }
            store::import::update_status(w.c(), &sid, &sname, hex_color(&state["color"]), &id)
                .await?;
            count(
                report,
                "statuses",
                if cur.0 == sname {
                    Change::Unchanged
                } else {
                    Change::Updated
                },
            );
            id
        }
        None => {
            let id = new_id();
            store::import::insert_status(
                w.c(),
                &id,
                team_id,
                &sname,
                kind,
                hex_color(&state["color"]),
                pos,
                &sid,
            )
            .await?;
            count(report, "statuses", Change::Created);
            id
        }
    };
    m.insert(sid, id);
    Ok(())
}

/// Imports the labels: groups first, then their children.
async fn import_labels(
    w: &mut Write<'_>,
    snap: &Snapshot,
    m: &mut HashMap<String, String>,
    report: &mut ImportReport,
) -> ApiResult<()> {
    let mut labels = snap.labels.clone();
    labels.sort_by_key(|l| !l["isGroup"].as_bool().unwrap_or(false));
    for l in &labels {
        import_label(w, l, m, report).await?;
    }
    Ok(())
}

async fn import_label(
    w: &mut Write<'_>,
    l: &Value,
    m: &mut HashMap<String, String>,
    report: &mut ImportReport,
) -> ApiResult<()> {
    let lid = s(&l["id"]).unwrap_or_default();
    let name = s(&l["name"]).unwrap_or_default().replace('/', "-");
    let is_group = l["isGroup"].as_bool().unwrap_or(false);
    let group = l["parent"]["id"].as_str().and_then(|p| m.get(p)).cloned();
    if l["parent"]["id"].is_string() && group.is_none() {
        report.problems.push(format!(
            "label {name:?} has a parent group that was not found; imported as a flat label"
        ));
    }
    let found = match find(w, "labels", &lid).await? {
        Some(id) => Some(id),
        None => store::import::find_label(w.c(), &group, &name).await?,
    };
    let id = match found {
        Some(id) => {
            store::import::update_label(
                w.c(),
                &lid,
                &name,
                hex_color(&l["color"]),
                s(&l["description"]),
                &group,
                is_group,
                &id,
            )
            .await?;
            count(report, "labels", Change::Unchanged);
            id
        }
        None => {
            let id = new_id();
            store::import::insert_label(
                w.c(),
                &id,
                &name,
                &group,
                is_group,
                hex_color(&l["color"]),
                s(&l["description"]),
                &lid,
            )
            .await?;
            count(report, "labels", Change::Created);
            id
        }
    };
    m.insert(lid, id);
    Ok(())
}

/// Imports one project, its teams and its milestones.
async fn import_project(
    w: &mut Write<'_>,
    p: &Value,
    m: &mut HashMap<String, String>,
    report: &mut ImportReport,
) -> ApiResult<()> {
    let lid = s(&p["id"]).unwrap_or_default();
    let id = upsert_project(w, p, &lid, report).await?;
    for team in p["teams"]["nodes"].as_array().into_iter().flatten() {
        if let Some(tid) = team["id"].as_str().and_then(|x| m.get(x)) {
            store::projects::add_team(w.c(), &id, tid.to_string()).await?;
        }
    }
    for (pos, ms) in p["projectMilestones"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        import_milestone(w, ms, &id, pos, m, report).await?;
    }
    m.insert(lid, id);
    Ok(())
}

/// Updates the project row for a Linear project, adopts a local project of
/// the same name, or creates one. Returns its ID.
async fn upsert_project(
    w: &mut Write<'_>,
    p: &Value,
    lid: &str,
    report: &mut ImportReport,
) -> ApiResult<String> {
    let name = s(&p["name"]).unwrap_or_else(|| "Project".into());
    let state = project_state(p["status"]["type"].as_str().unwrap_or("backlog"));
    let description = s(&p["description"]).unwrap_or_default();
    let id = match find(w, "projects", lid).await? {
        Some(id) => {
            store::import::update_project(
                w.c(),
                &name,
                &description,
                state,
                t(&p["archivedAt"]),
                &id,
            )
            .await?;
            count(report, "projects", Change::Unchanged);
            id
        }
        None => {
            let adopt: Option<String> = store::import::find_project_by_name(w.c(), &name).await?;
            match adopt {
                Some(id) => {
                    store::import::set_project_linear_id(w.c(), lid, &id).await?;
                    count(report, "projects", Change::Updated);
                    id
                }
                None => {
                    let id = new_id();
                    let mut slug = projects::slugify(&name);
                    let taken: Option<i64> =
                        store::import::project_slug_taken(w.c(), &slug).await?;
                    if taken.is_some() {
                        slug = format!("{slug}-{}", &lid[..lid.len().min(6)]);
                    }
                    store::import::insert_project(
                        w.c(),
                        &id,
                        &slug,
                        &name,
                        &description,
                        state,
                        lid,
                        t(&p["createdAt"]).unwrap_or_else(now),
                        t(&p["updatedAt"]).unwrap_or_else(now),
                        t(&p["archivedAt"]),
                    )
                    .await?;
                    count(report, "projects", Change::Created);
                    id
                }
            }
        }
    };
    Ok(id)
}

async fn import_milestone(
    w: &mut Write<'_>,
    ms: &Value,
    project_id: &str,
    pos: usize,
    m: &mut HashMap<String, String>,
    report: &mut ImportReport,
) -> ApiResult<()> {
    let mlid = s(&ms["id"]).unwrap_or_default();
    let mname = s(&ms["name"]).unwrap_or_else(|| "Milestone".into());
    let mid = match find(w, "milestones", &mlid).await? {
        Some(mid) => {
            store::import::update_milestone(
                w.c(),
                &mname,
                s(&ms["description"]).unwrap_or_default(),
                s(&ms["targetDate"]),
                &mid,
            )
            .await?;
            count(report, "milestones", Change::Unchanged);
            mid
        }
        None => {
            let mid = new_id();
            store::import::insert_milestone(
                w.c(),
                &mid,
                project_id,
                &mname,
                s(&ms["description"]).unwrap_or_default(),
                s(&ms["targetDate"]),
                pos as i64,
                &mlid,
            )
            .await?;
            count(report, "milestones", Change::Created);
            mid
        }
    };
    m.insert(mlid, mid);
    Ok(())
}

async fn import_document(
    w: &mut Write<'_>,
    ctx: &Ctx,
    d: &Value,
    m: &HashMap<String, String>,
    files: &HashMap<String, String>,
    report: &mut ImportReport,
) -> ApiResult<()> {
    let lid = s(&d["id"]).unwrap_or_default();
    let Some(project) = d["project"]["id"].as_str().and_then(|p| m.get(p)).cloned() else {
        return Ok(());
    };
    let title = s(&d["title"]).unwrap_or_else(|| "Untitled".into());
    let body = rewrite(&s(&d["content"]).unwrap_or_default(), files);
    match find(w, "documents", &lid).await? {
        Some(id) => {
            let (cur, version): (String, i64) = store::import::document_body(w.c(), &id).await?;
            if cur != body {
                store::import::update_document(w.c(), &title, &body, version + 1, &id).await?;
                crate::store::revisions::add_revision(
                    w.c(),
                    "document",
                    &id,
                    version + 1,
                    &body,
                    &ctx.actor_id(),
                    ctx.session(),
                )
                .await?;
                count(report, "documents", Change::Updated);
            } else {
                count(report, "documents", Change::Unchanged);
            }
        }
        None => {
            let id = new_id();
            let mut slug = projects::slugify(&title);
            let taken: Option<i64> =
                store::import::document_slug_taken(w.c(), &project, &slug).await?;
            if taken.is_some() {
                slug = format!("{slug}-{}", &lid[..lid.len().min(6)]);
            }
            store::import::insert_document(
                w.c(),
                &id,
                &project,
                &slug,
                &title,
                &body,
                &lid,
                ctx.actor_id(),
                t(&d["createdAt"]).unwrap_or_else(now),
                t(&d["updatedAt"]).unwrap_or_else(now),
                t(&d["archivedAt"]),
            )
            .await?;
            crate::store::revisions::add_revision(
                w.c(),
                "document",
                &id,
                1,
                &body,
                &ctx.actor_id(),
                ctx.session(),
            )
            .await?;
            count(report, "documents", Change::Created);
        }
    }
    Ok(())
}

/// Imports the issues, first without parents and relations. Issues changed
/// here since their last import keep the local version; their IDs come back.
async fn import_issues(
    w: &mut Write<'_>,
    ctx: &Ctx,
    snap: &Snapshot,
    m: &mut HashMap<String, String>,
    files: &HashMap<String, String>,
    report: &mut ImportReport,
) -> ApiResult<HashSet<String>> {
    let label_names: HashMap<String, String> = snap
        .labels
        .iter()
        .filter_map(|l| Some((s(&l["id"])?, s(&l["name"])?.to_lowercase())))
        .collect();
    let mut kept: HashSet<String> = HashSet::new();
    for i in &snap.issues {
        if let Some(id) = import_issue(w, ctx, i, m, files, &label_names, report).await? {
            kept.insert(id);
        }
    }
    Ok(kept)
}

/// Sets one issue's parent and adds its relations.
async fn link_issue(
    w: &mut Write<'_>,
    ctx: &Ctx,
    i: &Value,
    m: &HashMap<String, String>,
    kept: &HashSet<String>,
    report: &mut ImportReport,
) -> ApiResult<()> {
    let Some(id) = i["id"].as_str().and_then(|x| m.get(x)).cloned() else {
        return Ok(());
    };
    let parent = i["parent"]["id"].as_str().and_then(|p| m.get(p)).cloned();
    if kept.contains(&id) {
        return Ok(());
    }
    if i["parent"]["id"].is_string() && parent.is_none() {
        report.problems.push(format!(
            "{}: parent is outside the import",
            i["identifier"].as_str().unwrap_or("?")
        ));
    }
    store::import::set_parent(w.c(), &parent, &id).await?;
    for r in i["relations"]["nodes"].as_array().into_iter().flatten() {
        import_relation(w, ctx, i, r, &id, m, report).await?;
    }
    Ok(())
}

async fn import_relation(
    w: &mut Write<'_>,
    ctx: &Ctx,
    i: &Value,
    r: &Value,
    id: &str,
    m: &HashMap<String, String>,
    report: &mut ImportReport,
) -> ApiResult<()> {
    let rlid = s(&r["id"]).unwrap_or_default();
    let Some(other) = r["relatedIssue"]["id"]
        .as_str()
        .and_then(|x| m.get(x))
        .cloned()
    else {
        report.problems.push(format!(
            "{}: a relation points outside the import",
            i["identifier"].as_str().unwrap_or("?")
        ));
        return Ok(());
    };
    let kind = match r["type"].as_str().unwrap_or("related") {
        "blocks" => "blocks",
        "duplicate" => "duplicate",
        _ => "related",
    };
    if find(w, "relations", &rlid).await?.is_some() {
        count(report, "relations", Change::Unchanged);
        return Ok(());
    }
    let dup: Option<String> = store::import::find_relation(w.c(), kind, id, &other).await?;
    if let Some(d) = dup {
        store::import::link_relation(w.c(), &rlid, &d).await?;
        count(report, "relations", Change::Unchanged);
        return Ok(());
    }
    store::import::insert_relation(w.c(), kind, id, &other, &rlid, ctx.actor_id()).await?;
    count(report, "relations", Change::Created);
    Ok(())
}

/// True when someone other than the import wrote to the issue after the
/// import last did. Every write records an activity event, so the activity
/// log tells.
async fn changed_here(w: &mut Write<'_>, ctx: &Ctx, issue_id: &str) -> ApiResult<bool> {
    let actor = ctx.actor_id();
    let hit: Option<i64> = store::import::changed_here(w.c(), issue_id, &actor).await?;
    Ok(hit.is_some())
}

/// One Linear issue, read and resolved to local IDs.
struct IssueFields {
    lid: String,
    ident: String,
    team_id: String,
    status_id: String,
    number: i64,
    key: String,
    title: String,
    description: String,
    priority: i64,
    assignee: Option<String>,
    creator: Option<String>,
    project: Option<String>,
    milestone: Option<String>,
    created: String,
    updated: String,
    completed: Option<String>,
    deleted: Option<String>,
}

/// What the local database holds for a Linear issue.
enum ExistingIssue {
    Absent,
    Found(String),
    /// The key belongs to an issue made here, so the import skips it.
    Skipped,
}

async fn import_issue(
    w: &mut Write<'_>,
    ctx: &Ctx,
    i: &Value,
    m: &mut HashMap<String, String>,
    files: &HashMap<String, String>,
    label_names: &HashMap<String, String>,
    report: &mut ImportReport,
) -> ApiResult<Option<String>> {
    let Some(f) = read_issue_fields(w, i, m, files, report).await? else {
        return Ok(None);
    };
    let existing = match find_existing_issue(w, &f, report).await? {
        ExistingIssue::Absent => None,
        ExistingIssue::Found(id) => Some(id),
        ExistingIssue::Skipped => return Ok(None),
    };
    if let Some(id) = &existing
        && changed_here(w, ctx, id).await?
    {
        report.problems.push(format!(
            "{}: changed here since the last import; kept the local version",
            f.ident
        ));
        count(report, "issues", Change::Unchanged);
        m.insert(f.lid.clone(), id.clone());
        return Ok(Some(id.clone()));
    }
    let id = match existing {
        Some(id) => {
            update_issue(w, ctx, &f, &id, report).await?;
            id
        }
        None => insert_issue(w, ctx, &f, report).await?,
    };
    m.insert(f.lid.clone(), id.clone());
    set_waiting_on(w, i, &id, label_names).await?;
    replace_issue_labels(w, i, &id, m).await?;
    for a in i["attachments"]["nodes"].as_array().into_iter().flatten() {
        import_attachment(w, ctx, a, &id, report).await?;
    }
    for c in i["comments"]["nodes"].as_array().into_iter().flatten() {
        import_comment(w, ctx, c, &id, m, files, report).await?;
    }
    Ok(None)
}

/// Resolves the issue's team and status. Returns `None`, with a problem in
/// the report, when either one was not imported.
async fn read_issue_fields(
    w: &mut Write<'_>,
    i: &Value,
    m: &HashMap<String, String>,
    files: &HashMap<String, String>,
    report: &mut ImportReport,
) -> ApiResult<Option<IssueFields>> {
    let lid = s(&i["id"]).unwrap_or_default();
    let ident = s(&i["identifier"]).unwrap_or_default();
    let Some(team_id) = i["team"]["id"].as_str().and_then(|x| m.get(x)).cloned() else {
        report.problems.push(format!("{ident}: team not imported"));
        return Ok(None);
    };
    let Some(status_id) = i["state"]["id"].as_str().and_then(|x| m.get(x)).cloned() else {
        report.problems.push(format!("{ident}: status not found"));
        return Ok(None);
    };
    let number = i["number"].as_i64().unwrap_or(0);
    let team_key: String = store::import::team_key(w.c(), &team_id).await?;
    let key = format!("{team_key}-{number}");
    if ident != key {
        report.problems.push(format!("{ident}: imported as {key}"));
    }
    let title = s(&i["title"]).unwrap_or_else(|| "(no title)".into());
    let description = rewrite(&s(&i["description"]).unwrap_or_default(), files);
    let priority = i["priority"].as_i64().unwrap_or(0).clamp(0, 4);
    let lookup = |field: &str| i[field]["id"].as_str().and_then(|x| m.get(x)).cloned();
    let assignee = lookup("assignee");
    let creator = lookup("creator");
    let project = lookup("project");
    let milestone = lookup("projectMilestone");
    let created = t(&i["createdAt"]).unwrap_or_else(now);
    let updated = t(&i["updatedAt"]).unwrap_or_else(now);
    let completed = t(&i["completedAt"]).or_else(|| t(&i["canceledAt"]));
    let deleted = t(&i["archivedAt"]);
    Ok(Some(IssueFields {
        lid,
        ident,
        team_id,
        status_id,
        number,
        key,
        title,
        description,
        priority,
        assignee,
        creator,
        project,
        milestone,
        created,
        updated,
        completed,
        deleted,
    }))
}

async fn find_existing_issue(
    w: &mut Write<'_>,
    f: &IssueFields,
    report: &mut ImportReport,
) -> ApiResult<ExistingIssue> {
    if let Some(id) = find(w, "issues", &f.lid).await? {
        return Ok(ExistingIssue::Found(id));
    }
    let by_key: Option<(String, Option<String>)> =
        store::import::issue_by_key(w.c(), &f.key).await?;
    Ok(match by_key {
        // Adopt the key only from an issue that came from Linear.
        Some((id, Some(_))) => ExistingIssue::Found(id),
        Some((_, None)) => {
            report.problems.push(format!(
                "{}: {} belongs to an issue made here; skipped it",
                f.ident, f.key
            ));
            ExistingIssue::Skipped
        }
        None => ExistingIssue::Absent,
    })
}

async fn update_issue(
    w: &mut Write<'_>,
    ctx: &Ctx,
    f: &IssueFields,
    id: &str,
    report: &mut ImportReport,
) -> ApiResult<()> {
    type Cur = (
        String,
        String,
        i64,
        String,
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let cur: Cur = store::import::issue_fields(w.c(), id).await?;
    let same = cur.0 == f.title
        && cur.1 == f.description
        && cur.3 == f.status_id
        && cur.4 == f.priority
        && cur.5 == f.assignee
        && cur.6 == f.project
        && cur.7 == f.milestone
        && cur.8 == f.deleted;
    let version = if cur.1 != f.description {
        cur.2 + 1
    } else {
        cur.2
    };
    store::import::update_issue(
        w.c(),
        &f.lid,
        &f.title,
        &f.description,
        version,
        &f.status_id,
        f.priority,
        &f.assignee,
        &f.project,
        &f.milestone,
        &f.completed,
        &f.deleted,
        same,
        &f.updated,
        id,
    )
    .await?;
    if version != cur.2 {
        crate::store::revisions::add_revision(
            w.c(),
            "issue",
            id,
            version,
            &f.description,
            &ctx.actor_id(),
            ctx.session(),
        )
        .await?;
        crate::ops::questions::record_changed(w.c(), id, &f.description, version, &ctx.actor_id())
            .await?;
    }
    if cur.5 != f.assignee
        && let Some((claim_id, claim_actor, _)) = crate::ops::claims::live_claim(w, id).await?
        && f.assignee.as_deref() != Some(claim_actor.as_str())
    {
        crate::ops::claims::end_claim(w, id, &claim_id, "reassigned").await?;
    }
    if !same {
        // One event per update, so the app reloads the issue and the
        // next import sees where this one ended.
        let fields: Vec<&str> = [
            ("title", cur.0 != f.title),
            ("description", cur.1 != f.description),
            ("status", cur.3 != f.status_id),
            ("priority", cur.4 != f.priority),
            ("assignee", cur.5 != f.assignee),
            ("project", cur.6 != f.project),
            ("milestone", cur.7 != f.milestone),
            ("trash", cur.8 != f.deleted),
        ]
        .into_iter()
        .filter_map(|(f, changed)| changed.then_some(f))
        .collect();
        w.event(
            EntityType::Issue,
            id,
            Some(id),
            ActivityField::Imported,
            None,
            Some(json!({ "from": "linear", "key": f.ident, "fields": fields })),
        )
        .await?;
    }
    count(
        report,
        "issues",
        if same {
            Change::Unchanged
        } else {
            Change::Updated
        },
    );
    Ok(())
}

async fn insert_issue(
    w: &mut Write<'_>,
    ctx: &Ctx,
    f: &IssueFields,
    report: &mut ImportReport,
) -> ApiResult<String> {
    let id = new_id();
    store::import::insert_issue(
        w.c(),
        &id,
        &f.team_id,
        f.number,
        &f.key,
        &f.title,
        &f.description,
        &f.status_id,
        f.priority,
        &f.assignee,
        &f.project,
        &f.milestone,
        &f.lid,
        f.creator.clone().unwrap_or_else(|| ctx.actor_id()),
        &f.created,
        &f.updated,
        &f.completed,
        &f.deleted,
    )
    .await?;
    store::import::add_issue_key(w.c(), &f.key, &id).await?;
    store::teams::reserve_number(w.c(), f.number, &f.team_id).await?;
    crate::store::revisions::add_revision(
        w.c(),
        "issue",
        &id,
        1,
        &f.description,
        &ctx.actor_id(),
        ctx.session(),
    )
    .await?;
    crate::ops::questions::record_created(w.c(), &id, &f.description).await?;
    w.event(
        EntityType::Issue,
        &id,
        Some(&id),
        ActivityField::Imported,
        None,
        Some(json!({ "from": "linear", "key": f.ident })),
    )
    .await?;
    count(report, "issues", Change::Created);
    Ok(id)
}

/// The ready-for-agent and needs-owner labels become the waiting-on field.
async fn set_waiting_on(
    w: &mut Write<'_>,
    i: &Value,
    id: &str,
    label_names: &HashMap<String, String>,
) -> ApiResult<()> {
    let label_names: Vec<String> = i["labels"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l["id"].as_str())
        .filter_map(|lid| label_names.get(lid).cloned())
        .collect();
    let waiting = if label_names.iter().any(|n| {
        matches!(
            n.as_str(),
            "needs-owner" | "needs-input" | "needs-user-input" | "blocked-on-owner"
        )
    }) {
        Some("owner")
    } else if label_names.iter().any(|n| n == "ready-for-agent") {
        Some("agent")
    } else {
        None
    };
    let finished: Option<String> = store::import::finished_status_type(w.c(), id).await?;
    store::import::set_waiting_on(w.c(), if finished.is_some() { None } else { waiting }, id)
        .await?;
    Ok(())
}

/// Labels replace the whole set, so a label removed in Linear goes here too.
async fn replace_issue_labels(
    w: &mut Write<'_>,
    i: &Value,
    id: &str,
    m: &HashMap<String, String>,
) -> ApiResult<()> {
    let want: Vec<String> = i["labels"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l["id"].as_str().and_then(|x| m.get(x)).cloned())
        .collect();
    store::import::clear_issue_labels(w.c(), id).await?;
    for l in want {
        store::import::add_issue_label(w.c(), id, &l).await?;
    }
    Ok(())
}

async fn import_attachment(
    w: &mut Write<'_>,
    ctx: &Ctx,
    a: &Value,
    id: &str,
    report: &mut ImportReport,
) -> ApiResult<()> {
    let alid = s(&a["id"]).unwrap_or_default();
    let url = s(&a["url"]).unwrap_or_default();
    if url.is_empty() {
        return Ok(());
    }
    let kind = match a["sourceType"].as_str() {
        Some("github") if url.contains("/pull/") => "pull_request",
        Some("github") if url.contains("/commit/") => "commit",
        _ => crate::store::attachments::guess_kind(&url).as_str(),
    };
    let existing: Option<String> = match find(w, "attachments", &alid).await? {
        Some(x) => Some(x),
        None => store::attachments::id_by_url(w.c(), id, &url).await?,
    };
    match existing {
        Some(x) => {
            store::import::update_attachment(
                w.c(),
                &alid,
                s(&a["title"]),
                s(&a["subtitle"]),
                a["metadata"].to_string(),
                &x,
            )
            .await?;
            count(report, "attachments", Change::Unchanged);
        }
        None => {
            store::import::insert_attachment(
                w.c(),
                id,
                kind,
                &url,
                s(&a["title"]),
                s(&a["subtitle"]),
                if a["metadata"].is_object() {
                    a["metadata"].to_string()
                } else {
                    "{}".into()
                },
                &alid,
                ctx.actor_id(),
                t(&a["createdAt"]).unwrap_or_else(now),
            )
            .await?;
            count(report, "attachments", Change::Created);
        }
    }
    Ok(())
}

async fn import_comment(
    w: &mut Write<'_>,
    ctx: &Ctx,
    c: &Value,
    id: &str,
    m: &mut HashMap<String, String>,
    files: &HashMap<String, String>,
    report: &mut ImportReport,
) -> ApiResult<()> {
    let clid = s(&c["id"]).unwrap_or_default();
    let body = rewrite(&s(&c["body"]).unwrap_or_default(), files);
    let author = c["user"]["id"].as_str().and_then(|x| m.get(x)).cloned();
    match find(w, "comments", &clid).await? {
        Some(x) => {
            let (cur, version): (String, i64) = store::import::comment_body(w.c(), &x).await?;
            if cur != body {
                store::import::update_comment(
                    w.c(),
                    &body,
                    version + 1,
                    t(&c["updatedAt"]).unwrap_or_else(now),
                    &x,
                )
                .await?;
                crate::store::revisions::add_revision(
                    w.c(),
                    "comment",
                    &x,
                    version + 1,
                    &body,
                    &ctx.actor_id(),
                    ctx.session(),
                )
                .await?;
                count(report, "comments", Change::Updated);
            } else {
                count(report, "comments", Change::Unchanged);
            }
            m.insert(clid, x);
        }
        None => {
            let cid = new_id();
            let parent = c["parent"]["id"].as_str().and_then(|x| m.get(x)).cloned();
            store::import::insert_comment(
                w.c(),
                &cid,
                id,
                parent,
                author,
                &body,
                &clid,
                t(&c["createdAt"]).unwrap_or_else(now),
                t(&c["updatedAt"]).unwrap_or_else(now),
            )
            .await?;
            crate::store::revisions::add_revision(
                w.c(),
                "comment",
                &cid,
                1,
                &body,
                &ctx.actor_id(),
                ctx.session(),
            )
            .await?;
            count(report, "comments", Change::Created);
            m.insert(clid, cid);
        }
    }
    Ok(())
}

/// The whole import: read, copy files, write.
pub async fn run(
    st: &AppState,
    fetch: &impl Fetch,
    req: &LinearImportRequest,
) -> ApiResult<ImportReport> {
    let mut report = ImportReport {
        dry_run: req.dry_run,
        ..Default::default()
    };
    let snap = read_linear(fetch, &req.teams).await?;
    let files = if req.dry_run || req.skip_files {
        HashMap::new()
    } else {
        download_files(st, fetch, &snap, &mut report).await?
    };
    if !req.dry_run {
        report.backup = Some(
            db::backup(&st.pool(), &st.data_dir, "pre-import")
                .await?
                .display()
                .to_string(),
        );
    }
    apply(st, &snap, &files, req.dry_run, &mut report).await?;
    if snap.users.is_empty() {
        report.problems.push("Linear returned no users".into());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_upload_urls() {
        let text = "See ![shot](https://uploads.linear.app/a/b/c.png) and <https://uploads.linear.app/x/y>.";
        assert_eq!(
            linear_upload_urls(text),
            [
                "https://uploads.linear.app/a/b/c.png",
                "https://uploads.linear.app/x/y"
            ]
        );
    }
}
