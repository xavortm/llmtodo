//! GitHub sync by polling. The local server has no public URL, so it cannot
//! take webhooks.
//!
//! Each poll reads the pull requests and commits of every linked repo that
//! changed since the last poll. A PR attaches to an issue when its branch
//! matches a live claim branch, or when its branch or title holds an issue key.

use std::collections::BTreeSet;
use std::future::Future;
use std::time::Duration;

use jiff::Timestamp;
use llmtodo_api_types::{ClientContext, GithubReport, StatusType};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, Ctx, Write};
use crate::store::{self, actors, issues, teams};

pub struct Response {
    pub status: u16,
    pub etag: Option<String>,
    pub body: Value,
    /// Seconds until the rate limit resets, when it ran out.
    pub rate_reset: Option<i64>,
    /// The path of the next page, from the `Link` header.
    pub next: Option<String>,
}

pub trait GithubApi: Send + Sync {
    fn get(
        &self,
        path: &str,
        etag: Option<&str>,
    ) -> impl Future<Output = ApiResult<Response>> + Send;
}

pub struct GithubHttp {
    token: String,
    http: reqwest::Client,
}

impl GithubHttp {
    /// Uses the token of the GitHub CLI, so there is nothing new to set up.
    pub async fn from_gh() -> Result<Self, String> {
        let out = tokio::process::Command::new("gh")
            .args(["auth", "token"])
            .output()
            .await
            .map_err(|e| format!("cannot run `gh auth token`: {e}"))?;
        let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out.status.success() || token.is_empty() {
            return Err("no GitHub token; run `gh auth login`".into());
        }
        let http = reqwest::Client::builder()
            .user_agent("llmtodo")
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { token, http })
    }
}

impl GithubApi for GithubHttp {
    async fn get(&self, path: &str, etag: Option<&str>) -> ApiResult<Response> {
        let mut req = self
            .http
            .get(format!("https://api.github.com{path}"))
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json");
        if let Some(e) = etag {
            req = req.header("If-None-Match", e);
        }
        let res = req
            .send()
            .await
            .map_err(|e| ApiError::internal(format!("GitHub request failed: {e}")))?;
        let status = res.status().as_u16();
        let header = |n: &str| {
            res.headers()
                .get(n)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let etag = header("etag");
        let remaining = header("x-ratelimit-remaining").and_then(|v| v.parse::<i64>().ok());
        let reset = header("x-ratelimit-reset").and_then(|v| v.parse::<i64>().ok());
        let rate_reset = (remaining == Some(0) && matches!(status, 403 | 429))
            .then(|| reset.unwrap_or(0) - Timestamp::now().as_second());
        let next = next_link(header("link").as_deref());
        let body = if status == 304 {
            Value::Null
        } else {
            res.json().await.unwrap_or(Value::Null)
        };
        Ok(Response {
            status,
            etag,
            body,
            rate_reset,
            next,
        })
    }
}

/// The path of the `rel="next"` page in a GitHub `Link` header.
fn next_link(link: Option<&str>) -> Option<String> {
    link?.split(',').find_map(|part| {
        let (url, params) = part.trim().split_once(';')?;
        if !params.split(';').any(|p| p.trim() == "rel=\"next\"") {
            return None;
        }
        let url = url.trim().strip_prefix('<')?.strip_suffix('>')?;
        url.strip_prefix("https://api.github.com")
            .map(str::to_string)
    })
}

/// A walk stops after this many pages, so a broken `Link` header cannot
/// loop forever.
const MAX_PAGES: usize = 100;

/// What one walk over a paged GitHub list found.
struct Walk {
    items: Vec<Value>,
    /// The walk reached the last page or an item older than the last poll.
    /// Only then may the poll time move forward.
    complete: bool,
    /// The ETag of the first page, to store when the walk is complete.
    etag: Option<String>,
    /// Why the sync paused, such as a spent rate limit.
    paused: Option<String>,
}

/// Reads pages from `path` until `older` says an item is older than the
/// last poll, or there is no next page. Only the first page sends `etag`.
async fn walk(
    api: &impl GithubApi,
    repo: &str,
    path: String,
    etag: Option<&str>,
    older: impl Fn(&Value) -> bool,
) -> ApiResult<Walk> {
    let mut walk = Walk {
        items: Vec::new(),
        complete: false,
        etag: etag.map(str::to_string),
        paused: None,
    };
    let mut next = Some(path);
    let mut pages = 0;
    while let Some(path) = next.take() {
        let res = api.get(&path, if pages == 0 { etag } else { None }).await?;
        if let Some(wait) = res.rate_reset {
            walk.paused = Some(format!(
                "GitHub rate limit reached; it resets in {wait} seconds"
            ));
            return Ok(walk);
        }
        if res.status == 401 {
            walk.paused = Some("GitHub refused the token; run `gh auth login`".into());
            return Ok(walk);
        }
        if pages == 0 && res.status == 304 {
            walk.complete = true;
            return Ok(walk);
        }
        if res.status != 200 {
            tracing::warn!(repo, status = res.status, path, "GitHub request failed");
            return Ok(walk);
        }
        if pages == 0 {
            walk.etag = res.etag.clone();
        }
        pages += 1;
        for item in res.body.as_array().into_iter().flatten() {
            if older(item) {
                walk.complete = true;
                return Ok(walk);
            }
            walk.items.push(item.clone());
        }
        if pages >= MAX_PAGES && res.next.is_some() {
            tracing::warn!(repo, path, "GitHub walk stopped after {MAX_PAGES} pages");
            break;
        }
        next = res.next;
    }
    walk.complete = true;
    Ok(walk)
}

/// The last report, for `github status` and the app.
pub static LAST: Mutex<Option<GithubReport>> = Mutex::const_new(None);
/// One poll at a time.
static RUNNING: Mutex<()> = Mutex::const_new(());

struct Repo {
    project_id: String,
    repo: String,
    etag: Option<String>,
    last_polled_at: Option<String>,
}

/// Branch names that many unrelated sessions share, so a claim on one says
/// nothing about which PR belongs to it.
fn is_shared_branch(branch: &str, default_branch: Option<&str>) -> bool {
    matches!(branch, "" | "main" | "master" | "trunk" | "develop") || Some(branch) == default_branch
}

/// The issues a PR belongs to, as issue IDs.
///
/// A PR matches a claim branch only when the claim is live, the PR comes from
/// the linked repo itself and not from a fork, and the branch is not a shared
/// one such as `main`. An issue that already holds the PR keeps getting its
/// updates, so a merge still moves it after the claim ended. Keys named in the
/// branch or the title always match.
async fn issues_for(w: &mut Write<'_>, repo: &str, pr: &Value) -> ApiResult<BTreeSet<String>> {
    let branch = pr["head"]["ref"].as_str().unwrap_or_default();
    let title = pr["title"].as_str().unwrap_or_default();
    let url = pr["html_url"].as_str().unwrap_or_default();
    let head_repo = pr["head"]["repo"]["full_name"].as_str();
    let default_branch = pr["base"]["repo"]["default_branch"].as_str();
    let mut ids = BTreeSet::new();
    let same_repo = head_repo.is_some_and(|h| h.eq_ignore_ascii_case(repo));
    if same_repo && !is_shared_branch(branch, default_branch) {
        let claimed: Vec<String> = store::github::issues_claimed_on_branch(w.c(), branch).await?;
        ids.extend(claimed);
    }
    if !url.is_empty() {
        let held: Vec<String> = store::github::issues_with_attachment(w.c(), url).await?;
        ids.extend(held);
    }
    let mut keys: Vec<String> = llmtodo_core::find_keys_any_case(branch)
        .into_iter()
        .map(|m| m.key.to_string())
        .collect();
    keys.extend(
        llmtodo_core::find_keys(title)
            .into_iter()
            .map(|m| m.key.to_string()),
    );
    for k in keys {
        if let Ok(id) = issues::resolve_id(w.c(), &k).await {
            ids.insert(id);
        }
    }
    Ok(ids)
}

/// Moves an issue for a PR event, unless it is finished or the team turned
/// the move off.
async fn move_issue(
    w: &mut Write<'_>,
    issue_id: &str,
    merged: bool,
    report: &mut GithubReport,
) -> ApiResult<()> {
    let row = issues::row(w.c(), issue_id).await?;
    if row.deleted_at.is_some() {
        return Ok(());
    }
    let cur = teams::status_by_id(w.c(), &row.status_id).await?;
    if !cur.kind().is_open() {
        return Ok(());
    }
    let team = teams::team_row(w.c(), &row.team_id).await?;
    let Some(target_id) = (if merged {
        team.pr_merged_status_id
    } else {
        team.pr_open_status_id
    }) else {
        return Ok(());
    };
    if target_id == cur.id {
        return Ok(());
    }
    let target = teams::status_by_id(w.c(), &target_id).await?;
    // A PR opening never moves an issue backward, such as from In Review to
    // In Progress.
    if !merged
        && target.kind() == StatusType::Started
        && cur.kind() == StatusType::Started
        && cur.position >= target.position
    {
        return Ok(());
    }
    // A review status needs a review section, which only an agent can write.
    if target.review
        && !llmtodo_core::review_missing(row.description.as_deref().unwrap_or("")).is_empty()
    {
        return Ok(());
    }
    if crate::ops::issues::set_status(w, &row, &target).await? {
        report
            .moves
            .push(format!("{}: {} -> {}", row.key, cur.name, target.name));
    }
    Ok(())
}

pub async fn poll(st: &AppState, api: &impl GithubApi) -> ApiResult<GithubReport> {
    let _one = RUNNING.lock().await;
    let mut report = GithubReport {
        last_poll_at: Some(Timestamp::now()),
        ..Default::default()
    };
    let pool = st.pool();
    let repos: Vec<Repo> = store::github::repos(&mut *pool.acquire().await?)
        .await?
        .into_iter()
        .map(|(project_id, repo, etag, last_polled_at)| Repo {
            project_id,
            repo,
            etag,
            last_polled_at,
        })
        .collect();
    let actor = actors::system(&pool, "github").await?;
    let ctx = Ctx {
        client: ClientContext::default(),
        actor,
    };
    for repo in repos {
        report.repos.push(repo.repo.clone());
        let started = db::now();
        // Pull requests, newest update first, page by page until we pass the
        // last poll.
        let path = format!(
            "/repos/{}/pulls?state=all&sort=updated&direction=desc&per_page=50",
            repo.repo
        );
        let last = repo.last_polled_at.clone();
        let pr_walk = walk(api, &repo.repo, path, repo.etag.as_deref(), |pr| {
            let updated = pr["updated_at"]
                .as_str()
                .and_then(|t| t.parse::<Timestamp>().ok())
                .map(db::ts);
            matches!((&updated, &last), (Some(u), Some(l)) if u < l)
        })
        .await?;
        let commit_walk = if pr_walk.paused.is_none() {
            let since = repo
                .last_polled_at
                .as_deref()
                .unwrap_or("1970-01-01T00:00:00Z");
            let path = format!(
                "/repos/{}/commits?per_page=100&since={}",
                repo.repo,
                &since[..19.min(since.len())]
            );
            walk(api, &repo.repo, path, None, |_| false).await?
        } else {
            Walk {
                items: Vec::new(),
                complete: false,
                etag: None,
                paused: None,
            }
        };
        let prs = &pr_walk.items;
        let commits = &commit_walk.items;

        let mut w = Write::begin(st, &ctx).await?;
        for pr in prs {
            report.pull_requests_seen += 1;
            let branch = pr["head"]["ref"].as_str().unwrap_or_default();
            let title = pr["title"].as_str().unwrap_or_default();
            let url = pr["html_url"].as_str().unwrap_or_default();
            let merged = pr["merged_at"].is_string();
            let draft = pr["draft"].as_bool().unwrap_or(false);
            let state = if merged {
                "merged"
            } else {
                pr["state"].as_str().unwrap_or("open")
            };
            let meta = json!({ "number": pr["number"], "state": state, "draft": draft, "branch": branch, "repo": repo.repo });
            for issue_id in issues_for(&mut w, &repo.repo, pr).await? {
                let before: i64 = store::github::attachment_count(w.c(), &issue_id).await?;
                let label = format!("#{} {title}", pr["number"]);
                crate::ops::attachments::attach(
                    &mut w,
                    &issue_id,
                    url,
                    Some(&label),
                    Some(llmtodo_api_types::AttachmentKind::PullRequest),
                    "github",
                    meta.clone(),
                )
                .await?;
                let after: i64 = store::github::attachment_count(w.c(), &issue_id).await?;
                report.attached += (after - before) as u64;
                if merged {
                    move_issue(&mut w, &issue_id, true, &mut report).await?;
                } else if state == "open" && !draft {
                    move_issue(&mut w, &issue_id, false, &mut report).await?;
                }
            }
        }
        for c in commits {
            report.commits_seen += 1;
            let msg = c["commit"]["message"].as_str().unwrap_or_default();
            let first_line = msg.lines().next().unwrap_or_default();
            let url = c["html_url"].as_str().unwrap_or_default();
            let mut ids = BTreeSet::new();
            for m in llmtodo_core::find_keys(msg) {
                if let Ok(id) = issues::resolve_id(w.c(), &m.key.to_string()).await {
                    ids.insert(id);
                }
            }
            for issue_id in ids {
                let exists: Option<i64> =
                    store::github::has_attachment(w.c(), &issue_id, url).await?;
                if exists.is_none() {
                    let sha = c["sha"].as_str().unwrap_or_default();
                    let meta = json!({ "sha": sha, "repo": repo.repo });
                    crate::ops::attachments::attach(
                        &mut w,
                        &issue_id,
                        url,
                        Some(first_line),
                        Some(llmtodo_api_types::AttachmentKind::Commit),
                        "github",
                        meta,
                    )
                    .await?;
                    report.attached += 1;
                }
            }
        }
        // Move the poll time only when both walks reached the last poll, so
        // a cut-short walk is read again next time.
        if pr_walk.complete && commit_walk.complete {
            store::github::save_poll(w.c(), &pr_walk.etag, &started, &repo.project_id, &repo.repo)
                .await?;
        }
        w.commit().await?;
        if let Some(why) = pr_walk.paused.or(commit_walk.paused) {
            report.paused = Some(why);
            break;
        }
    }
    *LAST.lock().await = Some(report.clone());
    Ok(report)
}

pub async fn poll_with_gh(st: &AppState) -> ApiResult<GithubReport> {
    let repos: i64 = store::github::repo_count(&mut *st.pool().acquire().await?).await?;
    if repos == 0 {
        let r = GithubReport {
            paused: Some("no linked repos; run `llmtodo project link-repo <owner/name>`".into()),
            last_poll_at: Some(Timestamp::now()),
            ..Default::default()
        };
        *LAST.lock().await = Some(r.clone());
        return Ok(r);
    }
    match GithubHttp::from_gh().await {
        Ok(api) => poll(st, &api).await,
        Err(reason) => {
            let r = GithubReport {
                paused: Some(reason),
                last_poll_at: Some(Timestamp::now()),
                ..Default::default()
            };
            *LAST.lock().await = Some(r.clone());
            Ok(r)
        }
    }
}

/// Polls every `github_poll_seconds` while the server runs.
pub async fn poll_loop(st: AppState) {
    // Give startup a moment, then catch up on what changed while off.
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
        _ = st.shutdown.notified() => return,
    }
    loop {
        if st.primary()
            && let Err(e) = poll_with_gh(&st).await
        {
            tracing::warn!(error = %e.message, "GitHub poll failed");
        }
        let secs: u64 = db::get_setting(&st.pool(), "github_poll_seconds")
            .await
            .ok()
            .flatten()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(secs.max(60))) => {}
            _ = st.shutdown.notified() => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::next_link;

    #[test]
    fn reads_the_next_page_from_the_link_header() {
        let link = r#"<https://api.github.com/repositories/1/pulls?page=2>; rel="next", <https://api.github.com/repositories/1/pulls?page=9>; rel="last""#;
        assert_eq!(
            next_link(Some(link)).as_deref(),
            Some("/repositories/1/pulls?page=2")
        );
        let last_page = r#"<https://api.github.com/repositories/1/pulls?page=1>; rel="prev""#;
        assert_eq!(next_link(Some(last_page)), None);
        assert_eq!(
            next_link(Some(r#"<https://evil.example/x>; rel="next""#)),
            None
        );
        assert_eq!(next_link(None), None);
    }
}
