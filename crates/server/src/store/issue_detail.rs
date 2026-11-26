//! The full detail of one issue, with everything an agent needs.

use crate::store::backend::Conn;
use llmtodo_api_types::{
    Attachment, AttachmentKind, GitPr, IssueDetail, IssueGit, PreviewBuild, WorkSession,
};

use crate::error::ApiResult;
use crate::store::backend;
use crate::store::{attachments, comments, issues, relations, reviews, sessions, usage};

/// One issue with everything an agent needs.
pub async fn load(conn: &mut Conn, issue_id: &str) -> ApiResult<IssueDetail> {
    load_with(conn, issue_id, None).await
}

/// [`load`], with statuses and actors given when the caller has them, such
/// as a write that just loaded them (LLM-469).
pub async fn load_with(
    conn: &mut Conn,
    issue_id: &str,
    lookup: Option<&issues::Lookup>,
) -> ApiResult<IssueDetail> {
    // One lookup of statuses and actors serves every part below.
    let owned;
    let lookup = match lookup {
        Some(l) => l,
        None => {
            owned = issues::Lookup::load(conn).await?;
            &owned
        }
    };
    let actors = &lookup.actors;
    let statuses = &lookup.statuses;
    // The parts read only by the issue's ID. On Postgres, outside a
    // transaction, they run at once on their own connections (LLM-465).
    let (
        issue,
        aliases,
        children,
        (sessions, prices),
        attachments,
        git_row,
        relations,
        comments,
        reviews,
    ) = match conn.read_pool() {
        Some(pool) => {
            let c = || async { pool.acquire().await.map_err(crate::error::ApiError::from) };
            tokio::try_join!(
                async { issues::load_one_with(&mut c().await?, issue_id, true, lookup).await },
                async { aliases(&mut c().await?, issue_id).await },
                async { children(&mut c().await?, issue_id, statuses).await },
                async { sessions::for_issue(&mut c().await?, issue_id, actors).await },
                async { attachments::attachments(&mut c().await?, issue_id).await },
                async { git_row(&mut c().await?, issue_id).await },
                async { relations::relations(&mut c().await?, issue_id, statuses).await },
                async { comments::comments(&mut c().await?, issue_id, actors).await },
                async { reviews::for_issue_with(&mut c().await?, issue_id, actors).await },
            )?
        }
        None => (
            issues::load_one_with(conn, issue_id, true, lookup).await?,
            aliases(conn, issue_id).await?,
            children(conn, issue_id, statuses).await?,
            sessions::for_issue(conn, issue_id, actors).await?,
            attachments::attachments(conn, issue_id).await?,
            git_row(conn, issue_id).await?,
            relations::relations(conn, issue_id, statuses).await?,
            comments::comments(conn, issue_id, actors).await?,
            reviews::for_issue_with(conn, issue_id, actors).await?,
        ),
    };
    // An issue moved to another team keeps its old key as an alias.
    let aliases: Vec<String> = aliases
        .into_iter()
        .filter(|k| *k != issue.key.to_string())
        .collect();
    let mut total = std::collections::BTreeMap::new();
    for ws in &sessions {
        usage::merge(&mut total, &ws.usage.by_model);
    }
    let preview_build = preview_build(&git_row);
    let git = git(git_row, &attachments, &sessions);
    let preview_build = preview_build.map(|mut b| {
        b.stale = preview_stale(&b, git.branch.as_deref(), &sessions);
        b
    });
    Ok(IssueDetail {
        aliases,
        children,
        relations,
        attachments,
        git,
        comments,
        sessions,
        usage: usage::summary(total, &prices),
        reviews,
        preview_build,
        issue,
    })
}

/// The recorded preview build, not yet checked for staleness.
fn preview_build(r: &GitRow) -> Option<PreviewBuild> {
    Some(PreviewBuild {
        path: r.preview_build_path.clone()?,
        commit: r.preview_build_commit.clone()?,
        host: r.preview_build_host.clone(),
        built_at: crate::db::parse_ts(r.preview_build_at.as_deref().unwrap_or_default()),
        stale: false,
    })
}

/// Whether a newer `HEAD` of the build's branch exists. Only agent sessions
/// on the issue's branch, seen at or after the build, count: a session on
/// another branch or an older one says nothing about this build. Without
/// a branch, every session seen since the build counts.
fn preview_stale(b: &PreviewBuild, branch: Option<&str>, sessions: &[WorkSession]) -> bool {
    sessions
        .iter()
        .filter(|s| s.last_seen_at >= b.built_at)
        .filter(|s| branch.is_none() || s.branch.as_deref() == branch)
        .filter_map(|s| Some((s.last_seen_at, s.last_head.as_deref()?)))
        .max_by_key(|(at, _)| *at)
        .is_some_and(|(_, h)| !same_commit(h, &b.commit))
}

/// Whether two commit hashes name the same commit. Either may be short.
fn same_commit(a: &str, b: &str) -> bool {
    let n = a.len().min(b.len());
    n >= 7 && a[..n].eq_ignore_ascii_case(&b[..n])
}

/// Every key of the issue, the current one included.
async fn aliases(conn: &mut Conn, issue_id: &str) -> ApiResult<Vec<String>> {
    Ok(
        backend::query_scalar("SELECT key FROM issue_keys WHERE issue_id = ? ORDER BY key")
            .bind(issue_id)
            .fetch_all(&mut *conn)
            .await?,
    )
}

async fn children(
    conn: &mut Conn,
    issue_id: &str,
    statuses: &std::collections::HashMap<String, crate::store::teams::StatusRow>,
) -> ApiResult<Vec<llmtodo_api_types::IssueLink>> {
    let child_ids: Vec<String> = backend::query_scalar(
        "SELECT id FROM issues WHERE parent_id = ? AND deleted_at IS NULL ORDER BY number",
    )
    .bind(issue_id)
    .fetch_all(&mut *conn)
    .await?;
    issues::links(conn, &child_ids, statuses).await
}

async fn git_row(conn: &mut Conn, issue_id: &str) -> ApiResult<GitRow> {
    Ok(backend::query_as::<GitRow>(
        "SELECT git_branch, git_worktree, git_host, git_pr,
                preview_build_path, preview_build_commit, preview_build_host, preview_build_at
         FROM issues WHERE id = ?",
    )
    .bind(issue_id)
    .fetch_one(&mut *conn)
    .await?)
}

#[derive(sqlx::FromRow)]
struct GitRow {
    git_branch: Option<String>,
    git_worktree: Option<String>,
    git_host: Option<String>,
    git_pr: Option<String>,
    preview_build_path: Option<String>,
    preview_build_commit: Option<String>,
    preview_build_host: Option<String>,
    preview_build_at: Option<String>,
}

/// The git panel of an issue. The PR is the one set by hand, or else the
/// newest PR attachment that is still open, or else the newest one.
fn git(r: GitRow, attachments: &[Attachment], sessions: &[WorkSession]) -> IssueGit {
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
    IssueGit {
        branch: r.git_branch,
        worktree: r.git_worktree,
        host: r.git_host,
        pr,
        head,
        commits: Vec::new(),
        repo_url: None,
    }
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
