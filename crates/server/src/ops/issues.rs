//! Issue rules: create, update, trash and restore, status changes, and
//! the reads that carry a rule.

use std::collections::BTreeSet;

use llmtodo_api_types::*;
use serde_json::json;
use sqlx::SqliteConnection;

use super::analyzing::end_analyzing;
use super::attachments::attach;
use super::claims::{end_claim, live_claim};
use super::j;
use super::relations;
use super::revisions::apply_body_change;
use crate::db::{new_id, now};
use crate::error::{ApiError, ApiResult};
use crate::state::Write;
use crate::store::issues::{self, IssueRow};
use crate::store::revisions::{self, add_revision};
use crate::store::{self, activity, actors, deps, issue_detail, labels, projects, teams};

/// Runs an issue query. A saved view named in `q` gives the base filters.
pub async fn query(
    c: &mut SqliteConnection,
    actor: &Actor,
    mut q: IssueQuery,
) -> ApiResult<Page<Issue>> {
    if let Some(name) = q.view.take() {
        let stored = store::views::stored_query(c, &name)
            .await?
            .ok_or_else(|| ApiError::not_found("view", &name))?;
        let base: IssueQuery = serde_json::from_str(&stored)?;
        q = merge_query(base, q);
    }
    let (found, next) = issues::query(c, actor, &q).await?;
    let ids: Vec<String> = found.iter().map(|(id, _)| id.clone()).collect();
    let mut items = issues::load(c, &ids, q.include_description).await?;
    for (item, (_, score)) in items.iter_mut().zip(found.iter()) {
        item.score = *score;
    }
    Ok(Page {
        items,
        next_cursor: next,
    })
}

/// The open, unblocked issues under a root issue or inside a project.
pub async fn frontier(c: &mut SqliteConnection, q: &FrontierQuery) -> ApiResult<Vec<Issue>> {
    let root = match &q.issue {
        Some(r) => Some(issues::resolve_live(c, r).await?.id),
        None => None,
    };
    let project = match &q.project {
        Some(p) => Some(projects::resolve(c, p).await?.id),
        None => None,
    };
    let ids = issues::frontier(
        c,
        root.as_deref(),
        project.as_deref(),
        q.depth.unwrap_or(1),
        q.for_executor.as_ref(),
    )
    .await?;
    issues::load(c, &ids, q.include_description).await
}

/// The full detail of an issue by key, alias or ID.
pub async fn view(c: &mut SqliteConnection, r: &str) -> ApiResult<IssueDetail> {
    let id = issues::resolve_id(c, r).await?;
    issue_detail::load(c, &id).await
}

/// The dependency tree of an issue.
pub async fn deps(c: &mut SqliteConnection, r: &str, open: bool) -> ApiResult<DepNode> {
    let id = issues::resolve_id(c, r).await?;
    deps::tree(c, &id, open).await
}

/// The description revisions and the activity of an issue.
pub async fn history(c: &mut SqliteConnection, r: &str) -> ApiResult<History> {
    let id = issues::resolve_id(c, r).await?;
    Ok(History {
        revisions: revisions::revisions(c, "issue", &id).await?,
        activity: activity::for_issue(c, &id).await?,
    })
}

/// Creates an issue inside a write. `forced_number` is for the Linear
/// import, which keeps the original keys.
pub async fn create(
    w: &mut Write<'_>,
    req: CreateIssue,
    forced_number: Option<i64>,
) -> ApiResult<String> {
    let ctx = w.ctx;
    let title = req.title.trim().to_string();
    if title.is_empty() {
        return Err(ApiError::validation("title is empty"));
    }
    let team = match &req.team {
        Some(t) => teams::team_row(w.c(), t).await?,
        None => teams::default_team(w.c()).await?,
    };
    let status = match &req.status {
        Some(s) => teams::status_by_name(w.c(), &team.id, s).await?,
        None => match teams::first_of_type(w.c(), &team.id, StatusType::Unstarted).await? {
            Some(s) => s,
            None => teams::statuses(w.c(), Some(&team.id))
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| ApiError::validation("team has no statuses"))?,
        },
    };
    if status.review
        && let Some(e) = super::reviews::section_missing("The new issue", &req.description)
    {
        return Err(e);
    }
    let number: i64 = match forced_number {
        Some(n) => {
            store::teams::reserve_number(w.c(), n, &team.id).await?;
            n
        }
        None => store::teams::allocate_number(w.c(), &team.id).await?,
    };
    let key = format!("{}-{number}", team.key);
    let id = new_id();
    let t = now();
    let completed = (!status.kind().is_open()).then(|| t.clone());
    store::issues::insert(
        w.c(),
        &id,
        &team.id,
        number,
        &key,
        &title,
        &req.description,
        &status.id,
        req.priority.unwrap_or(Priority::None).as_number() as i64,
        ctx.actor_id(),
        &t,
        completed,
        serde_json::to_string(&req.wanted_executor.clone().unwrap_or_default())?,
    )
    .await?;
    store::issues::add_key(w.c(), &key, &id).await?;
    let actor = w.ctx.actor_id();
    let session = w.ctx.session().map(str::to_string);
    add_revision(
        w.c(),
        "issue",
        &id,
        1,
        &req.description,
        &actor,
        session.as_deref(),
    )
    .await?;
    super::questions::record_created(w.c(), &id, &req.description).await?;
    w.event(
        EntityType::Issue,
        &id,
        Some(&id),
        ActivityField::Created,
        None,
        j(json!({ "key": key, "title": title })),
    )
    .await?;

    let row = issues::row(w.c(), &id).await?;
    let update = UpdateIssue {
        assignee: Change::text(req.assignee),
        labels: (!req.labels.is_empty()).then_some(req.labels),
        project: Change::text(req.project),
        milestone: Change::text(req.milestone),
        parent: Change::text(req.parent),
        waiting_on: req.waiting_on.map_or(Change::Keep, Change::Set),
        estimate_minutes: req.estimate_minutes,
        estimate_cost_usd: req.estimate_cost_usd,
        ..Default::default()
    };
    apply_fields(w, &row, update, true).await?;
    for (kind, others) in req.relations {
        for other in others {
            relations::add(w, &id, kind, &other).await?;
        }
    }
    Ok(id)
}

pub async fn update(w: &mut Write<'_>, r: &str, req: UpdateIssue) -> ApiResult<String> {
    let row = issues::resolve_live(w.c(), r).await?;
    apply_fields(w, &row, req, false).await?;

    Ok(row.id)
}

pub async fn delete(w: &mut Write<'_>, r: &str) -> ApiResult<String> {
    let row = issues::resolve_live(w.c(), r).await?;
    store::issues::trash(w.c(), &row.id).await?;
    if let Some((claim_id, _, _)) = live_claim(w, &row.id).await? {
        end_claim(w, &row.id, &claim_id, "deleted").await?;
    }
    end_analyzing(w, &row.id, "deleted").await?;
    w.event(
        EntityType::Issue,
        &row.id,
        Some(&row.id),
        ActivityField::Deleted,
        None,
        None,
    )
    .await?;

    Ok(row.id)
}

pub async fn restore(w: &mut Write<'_>, r: &str, req: RestoreRequest) -> ApiResult<String> {
    let id = issues::resolve_id(w.c(), r).await?;
    let row = issues::row(w.c(), &id).await?;
    match req.revision {
        None => {
            if row.deleted_at.is_none() {
                return Err(ApiError::validation(format!(
                    "{} is not in the trash",
                    row.key
                )));
            }
            store::issues::untrash(w.c(), &id).await?;
            w.event(
                EntityType::Issue,
                &id,
                Some(&id),
                ActivityField::Restored,
                None,
                None,
            )
            .await?;
        }
        Some(v) => {
            if row.deleted_at.is_some() {
                return Err(ApiError::validation(format!(
                    "{} is in the trash; restore it first",
                    row.key
                )));
            }
            let body = revisions::revision_body(w.c(), "issue", &id, v).await?;
            let change = BodyChange {
                body: Some(body),
                force: true,
                ..Default::default()
            };
            if change_description(w, &row, &change).await? {
                touch_updated(w, &id).await?;
            }
        }
    }

    Ok(id)
}

/// Filters set in `extra` narrow the saved `base` query.
fn merge_query(mut base: IssueQuery, extra: IssueQuery) -> IssueQuery {
    base.teams.extend(extra.teams);
    base.projects.extend(extra.projects);
    base.filter.narrow(extra.filter);
    base.trashed |= extra.trashed;
    base.include_description |= extra.include_description;
    macro_rules! take {
        ($($f:ident),*) => { $( if extra.$f.is_some() { base.$f = extra.$f; } )* };
    }
    take!(
        parent,
        milestone,
        priority,
        for_executor,
        waiting_on,
        depends_on,
        text,
        similar,
        updated_after,
        order,
        cursor,
        limit
    );
    base
}

pub(super) async fn touch_updated(w: &mut Write<'_>, issue_id: &str) -> ApiResult<()> {
    store::issues::touch_updated(w.c(), issue_id).await?;
    w.touch(issue_id);
    Ok(())
}

/// Sets the branch, worktree and PR fields of an update. A worktree takes
/// the caller's host. A PR is attached too, so the GitHub sync follows it.
/// Returns whether anything changed.
async fn apply_git(w: &mut Write<'_>, id: &str, req: &UpdateIssue) -> ApiResult<bool> {
    let (branch, worktree, host, pr): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = store::issues::git_fields(w.c(), id).await?;
    let mut changed = false;
    if let Some(b) = req.branch.as_deref() {
        let new = b.map(str::to_string);
        if new != branch {
            store::issues::set_git_branch(w.c(), new.as_deref(), id).await?;
            w.event(
                EntityType::Issue,
                id,
                Some(id),
                ActivityField::GitBranch,
                j(&branch),
                j(&new),
            )
            .await?;
            changed = true;
        }
    }
    if let Some(wt) = req.worktree.as_deref() {
        let new = wt.map(str::to_string);
        let new_host = new.as_ref().and(w.ctx.client.host.clone());
        if new != worktree || new_host != host {
            store::issues::set_git_worktree(w.c(), new.as_deref(), new_host.as_deref(), id).await?;
            if new != worktree {
                w.event(
                    EntityType::Issue,
                    id,
                    Some(id),
                    ActivityField::GitWorktree,
                    j(&worktree),
                    j(&new),
                )
                .await?;
            }
            changed = true;
        }
    }
    if let Some(p) = req.pr.as_deref() {
        let new = p.map(str::to_string);
        if let Some(url) = &new {
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err(ApiError::validation(format!(
                    "the PR must be a web URL, got {url:?}"
                )));
            }
            attach(
                w,
                id,
                url,
                None,
                Some(AttachmentKind::PullRequest),
                "manual",
                serde_json::Value::Object(Default::default()),
            )
            .await?;
        }
        if new != pr {
            store::issues::set_git_pr(w.c(), new.as_deref(), id).await?;
            w.event(
                EntityType::Issue,
                id,
                Some(id),
                ActivityField::GitPr,
                j(&pr),
                j(&new),
            )
            .await?;
            changed = true;
        }
    }
    Ok(changed)
}

/// Applies every set field of an update, in a fixed order: team move first,
/// so statuses resolve in the new team.
pub async fn apply_fields(
    w: &mut Write<'_>,
    row: &IssueRow,
    req: UpdateIssue,
    creating: bool,
) -> ApiResult<()> {
    let mut row = row.clone();
    let mut changed = false;
    let waiting_on_kept = req.waiting_on.is_keep();

    if let Some(team) = &req.team {
        changed |= apply_team(w, &mut row, team).await?;
    }
    if let Some(title) = &req.title {
        changed |= apply_title(w, &row, title).await?;
    }
    if let Some(p) = req.priority {
        changed |= apply_priority(w, &row, p).await?;
    }
    if let Some(wo) = req.waiting_on.clone().into_option() {
        changed |= apply_waiting_on(w, &row, wo, creating).await?;
    }
    if !(req.branch.is_keep() && req.worktree.is_keep() && req.pr.is_keep()) {
        changed |= apply_git(w, &row.id, &req).await?;
    }
    changed |= apply_estimates(w, &row, &req).await?;
    if let Some(we) = &req.wanted_executor {
        changed |= apply_executor(w, &row, we).await?;
    }
    if let Some(p) = req.project.as_deref() {
        changed |= apply_project(w, &mut row, p, !req.milestone.is_keep(), creating).await?;
    }
    if let Some(m) = req.milestone.as_deref() {
        changed |= apply_milestone(w, &row, m).await?;
    }
    if let Some(p) = req.parent.as_deref() {
        changed |= apply_parent(w, &row, p).await?;
    }
    if req.labels.is_some() || !req.add_labels.is_empty() || !req.remove_labels.is_empty() {
        changed |= apply_labels(w, &row.id, &req).await?;
    }
    if let Some(a) = req.assignee.as_deref() {
        changed |= apply_assignee(w, &row, a).await?;
    }
    // The description goes before the status, so one update can add the
    // review section and move the issue to User review.
    if !req.description.is_empty() {
        changed |= apply_description(w, &row, &req).await?;
    }
    if let Some(s) = &req.status {
        let status = teams::status_by_name(w.c(), &row.team_id, s).await?;
        let moved = set_status(w, &row, &status).await?;
        // An issue in a review status waits on the owner, unless the update
        // says otherwise.
        if moved && waiting_on_kept && status.review {
            let fresh = issues::row(w.c(), &row.id).await?;
            apply_waiting_on(w, &fresh, Some(WaitingOn::Owner), false).await?;
        }
        changed |= moved;
    }
    if changed && !creating {
        touch_updated(w, &row.id).await?;
    }
    Ok(())
}

/// Sets the estimates of an update. Filling in an empty estimate is free.
/// Changing one that is set needs a reason, and the activity log records
/// the old value, the new value and that reason.
async fn apply_estimates(w: &mut Write<'_>, row: &IssueRow, req: &UpdateIssue) -> ApiResult<bool> {
    let reason = req
        .estimate_reason
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    if req.estimate_minutes.is_none() && req.estimate_cost_usd.is_none() {
        if req.estimate_reason.is_some() {
            return Err(ApiError::validation(
                "an estimate reason needs a new estimate; pass --estimate-minutes or --estimate-cost",
            ));
        }
        return Ok(false);
    }
    if req.estimate_minutes == Some(0) {
        return Err(ApiError::validation("estimate_minutes must be above 0"));
    }
    if let Some(usd) = req.estimate_cost_usd
        && !(usd.is_finite() && usd > 0.0)
    {
        return Err(ApiError::validation("estimate_cost_usd must be above 0"));
    }
    let new_minutes = req
        .estimate_minutes
        .filter(|m| Some(i64::from(*m)) != row.estimate_minutes);
    let new_cost = req.estimate_cost_usd.filter(|usd| {
        row.estimate_cost_usd
            .is_none_or(|old| (old - usd).abs() > 1e-9)
    });
    let replaces_set_value = (new_minutes.is_some() && row.estimate_minutes.is_some())
        || (new_cost.is_some() && row.estimate_cost_usd.is_some());
    if replaces_set_value && reason.is_none() {
        return Err(ApiError::validation(format!(
            "{} already has estimates, and they change only when the spec changes. Pass --estimate-reason \"<what changed in the spec>\" to change them.",
            row.key
        ))
        .with_details(json!({
            "reason": "estimate_frozen",
            "estimate_minutes": row.estimate_minutes,
            "estimate_cost_usd": row.estimate_cost_usd,
        })));
    }
    if let Some(m) = new_minutes {
        store::issues::set_estimate_minutes(w.c(), m, &row.id).await?;
        w.event(
            EntityType::Issue,
            &row.id,
            Some(&row.id),
            ActivityField::EstimateMinutes,
            j(row.estimate_minutes),
            j(m),
        )
        .await?;
    }
    if let Some(usd) = new_cost {
        store::issues::set_estimate_cost(w.c(), usd, &row.id).await?;
        w.event(
            EntityType::Issue,
            &row.id,
            Some(&row.id),
            ActivityField::EstimateCostUsd,
            j(row.estimate_cost_usd),
            j(usd),
        )
        .await?;
    }
    if replaces_set_value {
        w.event(
            EntityType::Issue,
            &row.id,
            Some(&row.id),
            ActivityField::EstimateReason,
            None,
            j(reason),
        )
        .await?;
    }
    Ok(new_minutes.is_some() || new_cost.is_some())
}

/// Moves the issue to another team and reloads `row` with its new key.
async fn apply_team(w: &mut Write<'_>, row: &mut IssueRow, team: &str) -> ApiResult<bool> {
    let target = teams::team_row(w.c(), team).await?;
    if target.id != row.team_id {
        move_team(w, row, &target).await?;
        *row = issues::row(w.c(), &row.id).await?;
        return Ok(true);
    }
    Ok(false)
}

async fn apply_title(w: &mut Write<'_>, row: &IssueRow, title: &str) -> ApiResult<bool> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ApiError::validation("title is empty"));
    }
    if title != row.title {
        store::issues::set_title(w.c(), title, &row.id).await?;
        w.event(
            EntityType::Issue,
            &row.id,
            Some(&row.id),
            ActivityField::Title,
            j(&row.title),
            j(title),
        )
        .await?;
        return Ok(true);
    }
    Ok(false)
}

async fn apply_priority(w: &mut Write<'_>, row: &IssueRow, p: Priority) -> ApiResult<bool> {
    let old = Priority::from_number(row.priority).unwrap_or(Priority::None);
    if p != old {
        store::issues::set_priority(w.c(), p.as_number() as i64, &row.id).await?;
        w.event(
            EntityType::Issue,
            &row.id,
            Some(&row.id),
            ActivityField::Priority,
            j(old),
            j(p),
        )
        .await?;
        return Ok(true);
    }
    Ok(false)
}

pub(super) async fn apply_waiting_on(
    w: &mut Write<'_>,
    row: &IssueRow,
    new: Option<WaitingOn>,
    creating: bool,
) -> ApiResult<bool> {
    let old: Option<WaitingOn> = row.waiting_on.as_deref().and_then(|w| w.parse().ok());
    if new != old {
        store::issues::set_waiting_on(
            w.c(),
            new.map(llmtodo_api_types::WaitingOn::as_str),
            &row.id,
        )
        .await?;
        if !creating || new.is_some() {
            w.event(
                EntityType::Issue,
                &row.id,
                Some(&row.id),
                ActivityField::WaitingOn,
                j(old),
                j(new),
            )
            .await?;
        }
        return Ok(true);
    }
    Ok(false)
}

async fn apply_executor(w: &mut Write<'_>, row: &IssueRow, we: &WantedExecutor) -> ApiResult<bool> {
    let old = row.executor();
    if *we != old {
        store::issues::set_executor(w.c(), serde_json::to_string(we)?, &row.id).await?;
        w.event(
            EntityType::Issue,
            &row.id,
            Some(&row.id),
            ActivityField::WantedExecutor,
            j(old.to_string()),
            j(we.to_string()),
        )
        .await?;
        return Ok(true);
    }
    Ok(false)
}

/// Sets the project and records it on `row`, so a milestone in the same
/// update resolves in the new project.
async fn apply_project(
    w: &mut Write<'_>,
    row: &mut IssueRow,
    p: Option<&str>,
    keep_milestone: bool,
    creating: bool,
) -> ApiResult<bool> {
    let new = match p {
        Some(p) => Some(projects::resolve(w.c(), p).await?),
        None => None,
    };
    if new.as_ref().map(|p| &p.id) != row.project_id.as_ref() {
        store::issues::set_project(
            w.c(),
            new.as_ref().map(|p| p.id.clone()),
            keep_milestone,
            &row.id,
        )
        .await?;
        if !creating {
            w.event(
                EntityType::Issue,
                &row.id,
                Some(&row.id),
                ActivityField::Project,
                j(&row.project_slug),
                j(new.as_ref().map(|p| &p.slug)),
            )
            .await?;
        }
        row.project_id = new.map(|p| p.id);
        return Ok(true);
    }
    Ok(false)
}

async fn apply_milestone(w: &mut Write<'_>, row: &IssueRow, m: Option<&str>) -> ApiResult<bool> {
    let new = match m {
        Some(m) => {
            let ms = projects::resolve_milestone(w.c(), row.project_id.as_deref(), m).await?;
            if row.project_id.as_deref() != Some(ms.project_id.as_str()) {
                return Err(ApiError::validation(format!(
                    "milestone {:?} belongs to project {:?}, not this issue's project",
                    ms.name, ms.project_slug
                )));
            }
            Some(ms)
        }
        None => None,
    };
    if new.as_ref().map(|m| &m.id) != row.milestone_id.as_ref() {
        store::issues::set_milestone(w.c(), new.as_ref().map(|m| m.id.clone()), &row.id).await?;
        w.event(
            EntityType::Issue,
            &row.id,
            Some(&row.id),
            ActivityField::Milestone,
            j(&row.milestone_name),
            j(new.as_ref().map(|m| &m.name)),
        )
        .await?;
        return Ok(true);
    }
    Ok(false)
}

async fn apply_parent(w: &mut Write<'_>, row: &IssueRow, p: Option<&str>) -> ApiResult<bool> {
    let new = match p {
        Some(p) => {
            let parent = issues::resolve_live(w.c(), p).await?;
            if issues::is_ancestor(w.c(), &row.id, &parent.id).await? {
                return Err(ApiError::validation(format!(
                    "{} cannot be the parent of {}: that makes a cycle",
                    parent.key, row.key
                )));
            }
            Some(parent)
        }
        None => None,
    };
    if new.as_ref().map(|p| &p.id) != row.parent_id.as_ref() {
        store::issues::set_parent(w.c(), new.as_ref().map(|p| p.id.clone()), &row.id).await?;
        w.event(
            EntityType::Issue,
            &row.id,
            Some(&row.id),
            ActivityField::Parent,
            j(&row.parent_key),
            j(new.as_ref().map(|p| &p.key)),
        )
        .await?;
        if let Some(p) = &new {
            w.touch(&p.id);
        }
        return Ok(true);
    }
    Ok(false)
}

async fn apply_assignee(w: &mut Write<'_>, row: &IssueRow, a: Option<&str>) -> ApiResult<bool> {
    let new = match a {
        Some(a) => {
            let ctx = w.ctx;
            Some(actors::resolve_ref(w.c(), &ctx.actor, a).await?)
        }
        None => None,
    };
    set_assignee(w, row, new).await
}

/// Changes the description. An answer to the last open owner question hands
/// the issue back to the agents, unless the update also sets `waiting_on`.
async fn apply_description(
    w: &mut Write<'_>,
    row: &IssueRow,
    req: &UpdateIssue,
) -> ApiResult<bool> {
    let before: String = store::issues::description(w.c(), &row.id).await?;
    if !change_description(w, row, &req.description).await? {
        return Ok(false);
    }
    if req.waiting_on.is_keep() {
        hand_back(w, &row.id, &before).await?;
    }
    Ok(true)
}

async fn apply_labels(w: &mut Write<'_>, id: &str, req: &UpdateIssue) -> ApiResult<bool> {
    let current = labels::for_issue(w.c(), id).await?;
    let mut next: Vec<labels::LabelRow> = match &req.labels {
        Some(all) => {
            let mut v = Vec::new();
            for l in all {
                v.push(labels::resolve(w.c(), l).await?);
            }
            v
        }
        None => current.clone(),
    };
    for l in &req.remove_labels {
        let label = labels::resolve(w.c(), l).await?;
        next.retain(|x| x.id != label.id);
    }
    for l in &req.add_labels {
        let label = labels::resolve(w.c(), l).await?;
        if !next.iter().any(|x| x.id == label.id) {
            next.push(label);
        }
    }
    labels::check_groups(&next)?;
    let old: BTreeSet<String> = current.iter().map(|l| l.id.clone()).collect();
    let new: BTreeSet<String> = next.iter().map(|l| l.id.clone()).collect();
    if old == new {
        return Ok(false);
    }
    for gone in old.difference(&new) {
        store::labels::remove_from_issue(w.c(), id, gone).await?;
    }
    for added in new.difference(&old) {
        store::labels::add_to_issue(w.c(), id, added).await?;
    }
    let names = |v: &[labels::LabelRow]| {
        let mut n: Vec<String> = v
            .iter()
            .map(super::super::store::labels::LabelRow::full_name)
            .collect();
        n.sort();
        n
    };
    w.event(
        EntityType::Issue,
        id,
        Some(id),
        ActivityField::Labels,
        j(names(&current)),
        j(names(&next)),
    )
    .await?;
    Ok(true)
}

pub(super) async fn set_assignee(
    w: &mut Write<'_>,
    row: &IssueRow,
    new: Option<String>,
) -> ApiResult<bool> {
    let cur: Option<String> = store::issues::assignee(w.c(), &row.id).await?;
    if cur == new {
        return Ok(false);
    }
    store::issues::set_assignee(w.c(), new.as_deref(), &row.id).await?;
    let all = actors::all(w.c()).await?;
    let name = |id: &Option<String>| id.as_ref().and_then(|i| all.get(i)).map(|a| a.name.clone());
    w.event(
        EntityType::Issue,
        &row.id,
        Some(&row.id),
        ActivityField::Assignee,
        j(name(&cur)),
        j(name(&new)),
    )
    .await?;
    if let Some((claim_id, claim_actor, _)) = live_claim(w, &row.id).await?
        && new.as_deref() != Some(claim_actor.as_str())
    {
        end_claim(w, &row.id, &claim_id, "reassigned").await?;
    }
    Ok(true)
}

pub(super) fn is_duplicate_status(s: &teams::StatusRow) -> bool {
    s.name.eq_ignore_ascii_case("duplicate")
}

pub(crate) async fn set_status(
    w: &mut Write<'_>,
    row: &IssueRow,
    status: &teams::StatusRow,
) -> ApiResult<bool> {
    let cur_id: String = store::issues::status_id(w.c(), &row.id).await?;
    if cur_id == status.id {
        return Ok(false);
    }
    if status.team_id != row.team_id {
        return Err(ApiError::validation(format!(
            "status {:?} belongs to another team",
            status.name
        )));
    }
    if is_duplicate_status(status) {
        let has: Option<i64> = store::relations::first_duplicate_of(w.c(), &row.id).await?;
        if has.is_none() {
            return Err(ApiError::validation(format!(
                "{} needs a duplicate relation to the original first: `llmtodo issue relation add {} duplicate <KEY>`",
                row.key, row.key
            )));
        }
    }
    if status.review {
        let description = store::issues::description(w.c(), &row.id).await?;
        if let Some(e) = super::reviews::section_missing(&row.key, &description) {
            return Err(e);
        }
    }
    let cur = teams::status_by_id(w.c(), &cur_id).await?;
    let completed = (!status.kind().is_open()).then(now);
    if !status.kind().is_open() {
        // A finished issue waits on no one.
        let waited: Option<String> = store::issues::waiting_on(w.c(), &row.id).await?;
        if waited.is_some() {
            store::issues::clear_waiting_on(w.c(), &row.id).await?;
            w.event(
                EntityType::Issue,
                &row.id,
                Some(&row.id),
                ActivityField::WaitingOn,
                j(waited),
                None,
            )
            .await?;
        }
    }
    store::issues::set_status(w.c(), &status.id, completed, &row.id).await?;
    w.event(
        EntityType::Issue,
        &row.id,
        Some(&row.id),
        ActivityField::Status,
        j(&cur.name),
        j(&status.name),
    )
    .await?;
    if !status.kind().is_open()
        && let Some((claim_id, _, _)) = live_claim(w, &row.id).await?
    {
        end_claim(w, &row.id, &claim_id, "completed").await?;
    }
    if !status.kind().is_open() {
        end_analyzing(w, &row.id, "completed").await?;
    }
    // A review status hands the ticket to the owner, so the agent's claim
    // ends and the app does not show it as locked.
    if status.review
        && let Some((claim_id, _, _)) = live_claim(w, &row.id).await?
    {
        end_claim(w, &row.id, &claim_id, super::claims::REVIEW_END_REASON).await?;
    }
    Ok(true)
}

/// Hands an issue back to the agents when its last open owner question got
/// an answer: `waiting_on` moves from `owner` to `agent`.
async fn hand_back(w: &mut Write<'_>, id: &str, before: &str) -> ApiResult<()> {
    if llmtodo_core::open_questions(before).is_empty() {
        return Ok(());
    }
    let (after, waiting): (String, Option<String>) =
        store::issues::description_and_waiting_on(w.c(), id).await?;
    let qs = llmtodo_core::questions(&after);
    if qs.is_empty()
        || qs.iter().any(llmtodo_core::Question::is_open)
        || waiting.as_deref() != Some("owner")
    {
        return Ok(());
    }
    store::issues::hand_to_agent(w.c(), id).await?;
    w.event(
        EntityType::Issue,
        id,
        Some(id),
        ActivityField::WaitingOn,
        j(WaitingOn::Owner),
        j(WaitingOn::Agent),
    )
    .await?;
    Ok(())
}

async fn change_description(
    w: &mut Write<'_>,
    row: &IssueRow,
    change: &BodyChange,
) -> ApiResult<bool> {
    let (current, version): (String, i64) =
        store::issues::description_and_version(w.c(), &row.id).await?;
    let next = apply_body_change(&current, version, change)?;
    if next == current {
        return Ok(false);
    }
    let v = version + 1;
    store::issues::set_description(w.c(), &next, v, &row.id).await?;
    let actor = w.ctx.actor_id();
    let session = w.ctx.session().map(str::to_string);
    add_revision(
        w.c(),
        "issue",
        &row.id,
        v,
        &next,
        &actor,
        session.as_deref(),
    )
    .await?;
    super::questions::record_changed(w.c(), &row.id, &next, v, &actor).await?;
    w.event(
        EntityType::Issue,
        &row.id,
        Some(&row.id),
        ActivityField::Description,
        j(version),
        j(v),
    )
    .await?;
    Ok(true)
}

async fn move_team(w: &mut Write<'_>, row: &IssueRow, target: &teams::TeamRow) -> ApiResult<()> {
    let number: i64 = store::teams::allocate_number(w.c(), &target.id).await?;
    let key = format!("{}-{number}", target.key);
    let cur = teams::status_by_id(w.c(), &row.status_id).await?;
    let all = teams::statuses(w.c(), Some(&target.id)).await?;
    let status = all
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(&cur.name))
        .or_else(|| all.iter().find(|s| s.kind() == cur.kind()))
        .or(all.first())
        .ok_or_else(|| ApiError::validation("target team has no statuses"))?
        .clone();
    store::issues::move_team(w.c(), &target.id, number, &key, &status.id, &row.id).await?;
    store::issues::add_key(w.c(), &key, &row.id).await?;
    w.event(
        EntityType::Issue,
        &row.id,
        Some(&row.id),
        ActivityField::Key,
        j(&row.key),
        j(&key),
    )
    .await?;
    Ok(())
}
