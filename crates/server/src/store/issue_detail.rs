//! The full detail of one issue, with everything an agent needs.

use llmtodo_api_types::{Attachment, AttachmentKind, GitPr, IssueDetail, IssueGit, WorkSession};
use sqlx::SqliteConnection;

use crate::error::ApiResult;
use crate::store::{attachments, comments, issues, relations, reviews, sessions, usage};

/// One issue with everything an agent needs.
pub async fn load(conn: &mut SqliteConnection, issue_id: &str) -> ApiResult<IssueDetail> {
    // One lookup of statuses and actors serves every part below.
    let lookup = issues::Lookup::load(conn).await?;
    let actors = &lookup.actors;
    let issue = issues::load_one_with(conn, issue_id, true, &lookup).await?;
    let aliases: Vec<String> = sqlx::query_scalar(
        "SELECT key FROM issue_keys WHERE issue_id = ? AND key <> ? ORDER BY key",
    )
    .bind(issue_id)
    .bind(issue.key.to_string())
    .fetch_all(&mut *conn)
    .await?;
    let child_ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM issues WHERE parent_id = ? AND deleted_at IS NULL ORDER BY number",
    )
    .bind(issue_id)
    .fetch_all(&mut *conn)
    .await?;
    let children = issues::links(conn, &child_ids, &lookup.statuses).await?;
    let sessions = sessions::for_issue(conn, issue_id, actors).await?;
    let mut total = std::collections::BTreeMap::new();
    for ws in &sessions {
        usage::merge(&mut total, &ws.usage.by_model);
    }
    let prices = usage::prices(conn).await?;
    let attachments = attachments::attachments(conn, issue_id).await?;
    let git = git(conn, issue_id, &attachments, &sessions).await?;
    Ok(IssueDetail {
        aliases,
        children,
        relations: relations::relations(conn, issue_id, &lookup.statuses).await?,
        attachments,
        git,
        comments: comments::comments(conn, issue_id, actors).await?,
        sessions,
        usage: usage::summary(total, &prices),
        reviews: reviews::for_issue_with(conn, issue_id, actors).await?,
        issue,
    })
}

#[derive(sqlx::FromRow)]
struct GitRow {
    git_branch: Option<String>,
    git_worktree: Option<String>,
    git_host: Option<String>,
    git_pr: Option<String>,
}

/// The git panel of an issue. The PR is the one set by hand, or else the
/// newest PR attachment that is still open, or else the newest one.
async fn git(
    conn: &mut SqliteConnection,
    issue_id: &str,
    attachments: &[Attachment],
    sessions: &[WorkSession],
) -> ApiResult<IssueGit> {
    let r = sqlx::query_as::<_, GitRow>(
        "SELECT git_branch, git_worktree, git_host, git_pr FROM issues WHERE id = ?",
    )
    .bind(issue_id)
    .fetch_one(&mut *conn)
    .await?;
    let prs: Vec<&Attachment> = attachments
        .iter()
        .filter(|a| a.kind == AttachmentKind::PullRequest)
        .collect();
    let pr = match &r.git_pr {
        Some(url) => Some(
            prs.iter()
                .find(|a| &a.url == url)
                .map(|a| git_pr(a))
                .unwrap_or_else(|| GitPr {
                    url: url.clone(),
                    title: None,
                    number: None,
                    state: None,
                }),
        ),
        None => prs
            .iter()
            .rev()
            .map(|a| git_pr(a))
            .find(|p| matches!(p.state.as_deref(), None | Some("open" | "draft")))
            .or_else(|| prs.last().map(|a| git_pr(a))),
    };
    let head = sessions
        .iter()
        .filter(|s| s.last_head.is_some())
        .max_by_key(|s| s.last_seen_at)
        .and_then(|s| s.last_head.clone());
    Ok(IssueGit {
        branch: r.git_branch,
        worktree: r.git_worktree,
        host: r.git_host,
        pr,
        head,
        commits: Vec::new(),
        repo_url: None,
    })
}

fn git_pr(a: &Attachment) -> GitPr {
    let m = &a.metadata;
    let state = m["state"].as_str().map(|s| {
        if s == "open" && m["draft"].as_bool() == Some(true) {
            "draft".to_string()
        } else {
            s.to_string()
        }
    });
    GitPr {
        url: a.url.clone(),
        title: a.title.clone(),
        number: m["number"].as_i64(),
        state,
    }
}
